//! Server authority for sleeping bags: rename (hold-E wheel), pick-up
//! (tap E), and the "respawn at my bag" path the death screen offers.
//!
//! Bags are ordinary deployables placed through the standard placement
//! command; this module only owns the bag-specific interactions. The
//! respawn-options list rides [`ServerMessage::PlayerKilled`], built in
//! `combat.rs` via [`GameServer::respawn_bag_options`].

use crate::{
    game_balance::{
        SLEEPING_BAG_NAME_MAX_LEN, SLEEPING_BAG_RESPAWN_COOLDOWN_TICKS,
        SLEEPING_BAG_SHARED_COOLDOWN_RADIUS_M,
    },
    items::{DeployableKind, SLEEPING_BAG_ID},
    protocol::{
        ClientId, DeployedEntityId, ItemStack, MAX_HEALTH, PlayerState, RespawnBagOption,
        SERVER_TICK_RATE_HZ, ServerMessage, SleepingBagCommand, ToastKind, Vec3Net,
    },
    server::PlayerLifecycle,
};

use super::{DeliveryTarget, GameServer, ServerEnvelope, inventory::add_stack_to_inventory};

use crate::game_balance::LOOT_BAG_INTERACT_RANGE_M as BAG_INTERACT_RANGE_M;

impl GameServer {
    pub(super) fn apply_sleeping_bag_command(
        &mut self,
        client_id: ClientId,
        command: SleepingBagCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            SleepingBagCommand::Rename { id, name } => self.rename_bag(client_id, id, name),
            SleepingBagCommand::PickUp { id } => self.pick_up_bag(client_id, id),
        }
    }

    fn rename_bag(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
        name: String,
    ) -> Vec<ServerEnvelope> {
        if self.owned_bag_in_range(client_id, id).is_none() {
            return Vec::new();
        }
        // Drop control characters, then trim (a stripped control char can
        // expose trailing whitespace), then cap the length. An empty
        // result clears the custom name back to the default label.
        let stripped: String = name.chars().filter(|c| !c.is_control()).collect();
        let cleaned: String = stripped
            .trim()
            .chars()
            .take(SLEEPING_BAG_NAME_MAX_LEN)
            .collect();
        let Some(entity) = self.deployed_entity_mut(id) else {
            return Vec::new();
        };
        entity.label = (!cleaned.is_empty()).then_some(cleaned.clone());
        let text = if cleaned.is_empty() {
            "Sleeping bag name cleared".to_owned()
        } else {
            format!("Renamed to \"{cleaned}\"")
        };
        bag_toast(client_id, ToastKind::Success, text)
    }

    fn pick_up_bag(&mut self, client_id: ClientId, id: DeployedEntityId) -> Vec<ServerEnvelope> {
        if self.owned_bag_in_range(client_id, id).is_none() {
            return Vec::new();
        }
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        // The bag only leaves the world if the item actually fits.
        if add_stack_to_inventory(&mut client.inventory, ItemStack::new(SLEEPING_BAG_ID, 1))
            .is_some()
        {
            return bag_toast(client_id, ToastKind::Warning, "Inventory full".to_owned());
        }
        self.destroy_deployed_entity(id);
        bag_toast(
            client_id,
            ToastKind::Success,
            "Picked up sleeping bag".to_owned(),
        )
    }

    /// The dying player's placed bags, for the death-screen spawn options.
    /// Sorted by id so the list is stable across deaths.
    pub(super) fn respawn_bag_options(
        &self,
        account: crate::protocol::AccountId,
    ) -> Vec<RespawnBagOption> {
        let mut bags: Vec<RespawnBagOption> = self
            .deployed_entities
            .values()
            .filter(|entity| {
                matches!(entity.kind, DeployableKind::SleepingBag) && entity.owner == Some(account)
            })
            .map(|entity| RespawnBagOption {
                id: entity.id,
                name: entity
                    .label
                    .clone()
                    .unwrap_or_else(|| "Sleeping Bag".to_owned()),
                cooldown_seconds: self.bag_cooldown_seconds_left(entity.id),
            })
            .collect();
        bags.sort_by_key(|bag| bag.id);
        bags
    }

    /// Seconds until bag `id` accepts a respawn again, `0` when ready.
    /// Rounds the tail second up so an active cooldown never reads "0".
    fn bag_cooldown_seconds_left(&self, id: DeployedEntityId) -> u32 {
        let Some(until) = self.bag_respawn_cooldowns.get(&id).copied() else {
            return 0;
        };
        let ticks = until.saturating_sub(self.tick);
        (ticks as f32 / SERVER_TICK_RATE_HZ).ceil() as u32
    }

    /// Respawn at an owned sleeping bag. Same lifecycle rules as the
    /// random respawn; the spawn point is a collider-free spot beside the
    /// bag (the bag itself may be tucked against a wall).
    pub(super) fn apply_respawn_at_bag_command(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.lifecycle.is_dead() {
            return Vec::new();
        }
        let account = client.account_id;
        let Some(entity) = self.deployed_entities.get(&id) else {
            return bag_toast(client_id, ToastKind::Warning, "That bag is gone".to_owned());
        };
        if !matches!(entity.kind, DeployableKind::SleepingBag) || entity.owner != Some(account) {
            return Vec::new();
        }
        // Cooldown gate: a freshly-used bag (or one near a freshly-used bag)
        // can't chain-respawn a defender straight back into the same fight.
        // The player stays dead; the death screen still offers the random
        // respawn and any other ready bag.
        let cooldown = self.bag_cooldown_seconds_left(id);
        if cooldown > 0 {
            return bag_toast(
                client_id,
                ToastKind::Warning,
                format!("Bag ready in {}:{:02}", cooldown / 60, cooldown % 60),
            );
        }
        let bag_position = entity.position;

        // Probe a ring of offsets around the bag for a spot the player
        // capsule fits; fall back to standing on the bag itself (it's
        // soft).
        let grid = self.spawn_collision_grid();
        let spawn = [
            (1.2, 0.0),
            (-1.2, 0.0),
            (0.0, 1.2),
            (0.0, -1.2),
            (0.9, 0.9),
            (-0.9, -0.9),
        ]
        .into_iter()
        .map(|(dx, dz)| Vec3Net::new(bag_position.x + dx, 0.0, bag_position.z + dz))
        .find(|candidate| !crate::controller::player_overlaps_world(*candidate, &grid))
        .unwrap_or(Vec3Net::new(bag_position.x, 0.0, bag_position.z));

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        client.controller.position = spawn;
        client.controller.velocity = Vec3Net::ZERO;
        client.controller.health = MAX_HEALTH;
        client.controller.grounded = true;
        client.lifecycle = PlayerLifecycle::Alive;
        client.next_attack_tick = self.tick;
        client.next_gather_tick = self.tick;
        let yaw = client.controller.yaw;
        let pitch = client.controller.pitch;
        let last_processed_input = client.controller.last_processed_input;

        self.chunk_manager.update_player_chunk(client_id, spawn);

        // Put the used bag on cooldown, shared with every same-owner bag
        // within the cluster radius so a carpet of bags in one base is one
        // re-entry, not a respawn chain. A remote bag stays ready.
        let until = self.tick + SLEEPING_BAG_RESPAWN_COOLDOWN_TICKS;
        let cluster: Vec<DeployedEntityId> = self
            .deployed_entities
            .values()
            .filter(|entity| {
                matches!(entity.kind, DeployableKind::SleepingBag)
                    && entity.owner == Some(account)
                    && entity.position.within_horizontal_range(
                        bag_position,
                        SLEEPING_BAG_SHARED_COOLDOWN_RADIUS_M,
                    )
            })
            .map(|entity| entity.id)
            .collect();
        for bag_id in cluster {
            self.bag_respawn_cooldowns.insert(bag_id, until);
        }

        vec![ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::Correction(PlayerState {
                client_id,
                position: spawn,
                velocity: Vec3Net::ZERO,
                yaw,
                pitch,
                health: MAX_HEALTH,
                grounded: true,
                last_processed_input,
            }),
        }]
    }

    /// Ownership + range gate for bag interactions.
    fn owned_bag_in_range(&self, client_id: ClientId, id: DeployedEntityId) -> Option<()> {
        let client = self.clients.get(&client_id)?;
        let entity = self.deployed_entities.get(&id)?;
        if !matches!(entity.kind, DeployableKind::SleepingBag) {
            return None;
        }
        if entity.owner != Some(client.account_id) {
            return None;
        }
        client
            .controller
            .position
            .within_horizontal_range(entity.position, BAG_INTERACT_RANGE_M)
            .then_some(())
    }
}

fn bag_toast(client_id: ClientId, kind: ToastKind, text: String) -> Vec<ServerEnvelope> {
    super::toasts::toast(client_id, kind, text)
}
