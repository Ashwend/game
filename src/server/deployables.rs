//! Server-authoritative state for placed structures (workbenches,
//! furnaces, future deployables).
//!
//! Storage shape mirrors `resource_nodes`: a `HashMap<DeployedEntityId, _>`
//! owned by `GameServer`, with chunk membership tracked separately in the
//! chunk manager so AoI snapshots filter by visible chunk.
//!
//! Placement validation lives here so the server is the single source of
//! truth for "can this go here?" — the client only shows a best-guess
//! preview. The same `placement_validation` helpers run on save load to
//! drop entries that no longer fit (e.g. a deployable saved before the
//! world geometry shifted).

use std::collections::HashMap;

use crate::{
    crafting::RecipeStation,
    items::{
        DeployableKind, DeployableProfile, HANDS_TOOL, ItemId, ToolKind, item_definition,
        tool_damage_multiplier_pct,
    },
    protocol::{
        ClientId, DamageDeployableCommand, DeployedEntityId, PlaceDeployableCommand, ServerMessage,
        ToastKind, ToastMessage, Vec3Net,
    },
    world::WorldBlock,
};

use super::{DeliveryTarget, GameServer, ServerEnvelope, inventory::take_items_from_inventory};

/// Maximum range, in metres, at which a player can damage a placed
/// structure. Matches the furnace open-range so the swing-tooltip-damage
/// flow stays consistent — if E reaches it, your tool reaches it too.
const DAMAGE_RANGE_M: f32 = 5.5;
/// Per-tool damage scalar. The tool's `gather_amount` already scales
/// with tier (stone tools = 6, future iron tools = higher), so
/// re-using it as the base means deployable damage tracks tool tier
/// without a separate balance table. The multiplier puts stone-tool
/// time-to-destroy in the survival-game-sweet-spot (~15 swings for a
/// workbench).
const DAMAGE_PER_GATHER_POINT: u32 = 5;

/// Maximum distance, in metres, from the placing player's feet to the
/// requested placement position. Keeps placements within arm's reach +
/// a forgiving margin for foot-of-camera vs centre-of-feet projection.
const PLACEMENT_REACH_M: f32 = 5.0;

/// Authoritative record of a placed structure. The id is server-assigned
/// and stable for the entity's lifetime.
#[derive(Debug, Clone)]
pub(crate) struct DeployedEntity {
    pub(super) id: DeployedEntityId,
    pub(super) item_id: ItemId,
    pub(super) kind: DeployableKind,
    pub(super) position: Vec3Net,
    pub(super) yaw: f32,
    pub(super) health: u32,
    pub(super) max_health: u32,
    /// Furnace-only state. `None` for non-furnaces; the place handler
    /// initialises a default `FurnaceState` for placed furnaces.
    pub(super) furnace: Option<super::furnace::FurnaceState>,
}

impl DeployedEntity {
    /// AABB for placement-overlap and (future) collision use. Same
    /// half-extents as the client builds, so the two stay aligned.
    pub(super) fn collider(&self, profile: DeployableProfile) -> WorldBlock {
        let center = Vec3Net::new(
            self.position.x,
            self.position.y + profile.collider_half_height,
            self.position.z,
        );
        let half = Vec3Net::new(
            profile.collider_half_width,
            profile.collider_half_height,
            profile.collider_half_width,
        );
        WorldBlock::new(center, half)
    }
}

impl GameServer {
    pub(super) fn apply_place_deployable_command(
        &mut self,
        client_id: ClientId,
        command: PlaceDeployableCommand,
    ) -> Vec<ServerEnvelope> {
        let Some(definition) = item_definition(&command.item_id) else {
            return place_toast(client_id, ToastKind::Error, "Unknown item".to_owned());
        };
        let Some(profile) = definition.deployable else {
            return place_toast(
                client_id,
                ToastKind::Warning,
                format!("{} can't be placed", definition.name),
            );
        };

        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };

        // Reach check: feet-to-target distance must not exceed
        // PLACEMENT_REACH_M, and the target must be at the world floor
        // (y≈0) so the player can't snipe a structure onto a rooftop.
        let feet = client.controller.position;
        let dx = command.position.x - feet.x;
        let dz = command.position.z - feet.z;
        if (dx * dx + dz * dz).sqrt() > PLACEMENT_REACH_M {
            return place_toast(client_id, ToastKind::Warning, "Too far away".to_owned());
        }
        if command.position.y.abs() > 0.25 {
            return place_toast(
                client_id,
                ToastKind::Warning,
                "Place on level ground".to_owned(),
            );
        }
        if !command.position.x.is_finite()
            || !command.position.y.is_finite()
            || !command.position.z.is_finite()
            || !command.yaw.is_finite()
        {
            return place_toast(client_id, ToastKind::Error, "Invalid placement".to_owned());
        }

        // Overlap check: a candidate AABB at the requested pose mustn't
        // intersect any other placed structure's AABB. Drop overlap test
        // is left to gather (drops sit lower than typical deployables);
        // resource nodes already enforce their own collision so the
        // player can't hammer a workbench inside a tree.
        let candidate = DeployedEntity {
            id: 0,
            item_id: command.item_id.clone(),
            kind: profile.kind,
            position: command.position,
            yaw: command.yaw,
            health: profile.max_health,
            max_health: profile.max_health,
            furnace: None,
        };
        let candidate_block = candidate.collider(profile);
        for existing in self.deployed_entities.values() {
            let Some(existing_def) = item_definition(&existing.item_id) else {
                continue;
            };
            let Some(existing_profile) = existing_def.deployable else {
                continue;
            };
            if blocks_overlap(candidate_block, existing.collider(existing_profile)) {
                return place_toast(
                    client_id,
                    ToastKind::Warning,
                    "Something is in the way".to_owned(),
                );
            }
        }

        // Recipe-station-style gating *of placement itself* is intentionally
        // not enforced here — gating happens at crafting time. A player who
        // somehow has a furnace in inventory (admin spawn, future trade)
        // can still place it without owning a workbench.

        // Consume one item from the player's inventory. Re-borrow mutably
        // now that the immutable client reference is no longer live.
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let removed = take_items_from_inventory(&mut client.inventory, definition.id, 1);
        if removed != 1 {
            return place_toast(
                client_id,
                ToastKind::Warning,
                format!("You don't have a {}", definition.name),
            );
        }

        let id = self.next_deployed_entity_id;
        self.next_deployed_entity_id = self.next_deployed_entity_id.saturating_add(1);
        let mut entity = DeployedEntity { id, ..candidate };
        // Furnaces ship with an empty operational state so the client
        // can render the slot grid the moment the entity appears in
        // the snapshot. Non-furnace deployables stay `None`.
        if matches!(entity.kind, DeployableKind::Furnace { .. }) {
            entity.furnace = Some(super::furnace::FurnaceState::default());
        }
        let position = entity.position;
        self.deployed_entities.insert(id, entity);
        self.chunk_manager.track_deployed_entity(id, position);

        place_toast(
            client_id,
            ToastKind::Success,
            format!("Placed {}", definition.name),
        )
    }

    pub(super) fn apply_damage_deployable_command(
        &mut self,
        client_id: ClientId,
        command: DamageDeployableCommand,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        // Honour the same per-tool cooldown that gathering uses so a
        // damage swing can't fire faster than the tool's swing cadence.
        if self.tick < client.next_gather_tick {
            return Vec::new();
        }
        let player_pos = client.controller.position;
        let tool = client
            .inventory
            .active_actionbar_stack()
            .and_then(|stack| item_definition(&stack.item_id))
            .and_then(|def| def.tool)
            .unwrap_or(HANDS_TOOL);
        // Bare hands don't damage placed structures — the client gates
        // this too, but defence in depth.
        if tool.kind == ToolKind::Hands {
            return Vec::new();
        }

        let Some(entity) = self.deployed_entities.get(&command.id) else {
            return Vec::new();
        };
        let dx = entity.position.x - player_pos.x;
        let dz = entity.position.z - player_pos.z;
        if (dx * dx + dz * dz).sqrt() > DAMAGE_RANGE_M {
            return Vec::new();
        }
        // Tool-vs-material multiplier — hatchet eats wood, pickaxe
        // eats stone, mismatched proper tools still chip away but at
        // ~1/3 the rate of the matched pairing.
        let multiplier_pct = tool_damage_multiplier_pct(tool.kind, entity.kind.material());
        let base = (tool.gather_amount as u32).saturating_mul(DAMAGE_PER_GATHER_POINT);
        let damage = base.saturating_mul(multiplier_pct) / 100;

        // Mutable borrow for the actual decrement. We re-fetch instead
        // of holding the earlier `entity` reference across the cooldown
        // write below — borrow-checker convenience, not a hot path.
        let Some(entity) = self.deployed_entities.get_mut(&command.id) else {
            return Vec::new();
        };
        entity.health = entity.health.saturating_sub(damage);
        let dead = entity.health == 0;

        // Apply the swing cooldown after a successful hit so spamming
        // damage swings doesn't bypass the gather throttle.
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.next_gather_tick = self.tick + tool.cooldown_ticks.max(1);
        }

        if dead {
            self.destroy_deployed_entity(command.id);
        }
        // Survivor health change replicates via the ECS mirror →
        // Lightyear's `DeployableHealth` diff. See
        // [Networking § Replication](../../docs/networking.md#replication).
        Vec::new()
    }

    /// Remove a placed structure entirely (gameplay death + tracker
    /// untrack). Players who had it open as a furnace get kicked back
    /// to the world view automatically because the snapshot's
    /// `open_furnace` view stops resolving once the entity is gone.
    pub(super) fn destroy_deployed_entity(&mut self, id: DeployedEntityId) {
        if self.deployed_entities.remove(&id).is_none() {
            return;
        }
        self.chunk_manager.untrack_deployed_entity(id);
        // Clear any client's open-furnace pointer at this id so they
        // don't keep trying to operate a destroyed entity.
        for client in self.clients.values_mut() {
            if client.open_furnace == Some(id) {
                client.open_furnace = None;
            }
        }
    }

    /// True when the player has any placed deployable in range that
    /// satisfies `station`. Used by `enqueue_craft` to gate recipes
    /// behind workbench/furnace presence.
    pub(super) fn station_in_range(&self, client_id: ClientId, station: RecipeStation) -> bool {
        if matches!(station, RecipeStation::None) {
            return true;
        }
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        let player_pos = client.controller.position;
        for entity in self.deployed_entities.values() {
            if !station.satisfied_by(entity.kind) {
                continue;
            }
            let Some(profile) = item_definition(&entity.item_id).and_then(|def| def.deployable)
            else {
                continue;
            };
            let dx = entity.position.x - player_pos.x;
            let dz = entity.position.z - player_pos.z;
            if dx * dx + dz * dz <= profile.station_radius * profile.station_radius {
                return true;
            }
        }
        false
    }

    /// Build the load-time map from a list of persisted deployable
    /// entries. Drops entries whose item id no longer resolves so a
    /// retired item type doesn't crash the load.
    pub(super) fn restore_deployed_entities(
        persisted: Vec<crate::save::PersistedDeployedEntity>,
    ) -> HashMap<DeployedEntityId, DeployedEntity> {
        persisted
            .into_iter()
            .filter_map(|p| {
                let item_id = crate::items::intern_item_id(&p.item_id);
                item_definition(&item_id)?;
                let furnace = p.furnace.map(super::furnace::FurnaceState::from_persisted);
                Some((
                    p.id,
                    DeployedEntity {
                        id: p.id,
                        item_id,
                        kind: p.kind,
                        position: p.position,
                        yaw: p.yaw,
                        health: p.health,
                        max_health: p.max_health,
                        furnace,
                    },
                ))
            })
            .collect()
    }

    /// Convert the live map back into save records. Order is sorted by
    /// id so save files diff cleanly across reloads.
    pub(super) fn persisted_deployed_entities(&self) -> Vec<crate::save::PersistedDeployedEntity> {
        let mut entries: Vec<_> = self
            .deployed_entities
            .values()
            .map(|entity| crate::save::PersistedDeployedEntity {
                id: entity.id,
                item_id: entity.item_id.as_ref().to_owned(),
                kind: entity.kind,
                position: entity.position,
                yaw: entity.yaw,
                health: entity.health,
                max_health: entity.max_health,
                furnace: entity.furnace.as_ref().map(|f| f.to_persisted()),
            })
            .collect();
        entries.sort_by_key(|entry| entry.id);
        entries
    }
}

fn place_toast(client_id: ClientId, kind: ToastKind, text: String) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(kind, text)),
    }]
}

fn blocks_overlap(a: WorldBlock, b: WorldBlock) -> bool {
    let a_min = a.min();
    let a_max = a.max();
    let b_min = b.min();
    let b_max = b.max();
    a_min.x < b_max.x
        && a_max.x > b_min.x
        && a_min.y < b_max.y
        && a_max.y > b_min.y
        && a_min.z < b_max.z
        && a_max.z > b_min.z
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        items::{CRUDE_FURNACE_ID, WORKBENCH_T1_ID, intern_item_id},
        protocol::{GAME_VERSION, ItemStack, PROTOCOL_VERSION},
        save::WorldSave,
        server::{ServerSettings, inventory::add_stack_to_inventory},
        steam::{AuthMode, offline_auth_token},
    };

    fn make_server() -> GameServer {
        GameServer::new(
            WorldSave::new("Test", Some(1)),
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: Some(1),
            },
        )
    }

    fn connect_test_client(server: &mut GameServer) -> ClientId {
        server
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                1,
                "Tester".to_owned(),
                offline_auth_token(1),
            )
            .expect("connect ok")
            .0
    }

    fn give_one(server: &mut GameServer, client_id: ClientId, item_id: &str) {
        let client = server.clients.get_mut(&client_id).unwrap();
        add_stack_to_inventory(&mut client.inventory, ItemStack::new(item_id, 1));
    }

    #[test]
    fn placing_a_workbench_consumes_one_item_and_tracks_chunk() {
        let mut server = make_server();
        let client_id = connect_test_client(&mut server);
        give_one(&mut server, client_id, WORKBENCH_T1_ID);

        let envelopes = server.apply_place_deployable_command(
            client_id,
            PlaceDeployableCommand {
                item_id: intern_item_id(WORKBENCH_T1_ID),
                position: Vec3Net::new(1.5, 0.0, 0.0),
                yaw: 0.0,
            },
        );
        assert!(
            envelopes
                .iter()
                .any(|e| matches!(&e.message, ServerMessage::Toast(t) if matches!(t.kind, ToastKind::Success))),
            "expected success toast, got {envelopes:?}"
        );
        assert_eq!(server.deployed_entities.len(), 1);
    }

    #[test]
    fn placement_out_of_reach_is_rejected() {
        let mut server = make_server();
        let client_id = connect_test_client(&mut server);
        give_one(&mut server, client_id, WORKBENCH_T1_ID);

        let envelopes = server.apply_place_deployable_command(
            client_id,
            PlaceDeployableCommand {
                item_id: intern_item_id(WORKBENCH_T1_ID),
                position: Vec3Net::new(50.0, 0.0, 0.0),
                yaw: 0.0,
            },
        );
        assert!(
            envelopes.iter().any(|e| matches!(&e.message, ServerMessage::Toast(t) if matches!(t.kind, ToastKind::Warning))),
            "expected warning toast for out-of-reach"
        );
        assert!(server.deployed_entities.is_empty());
        // No item consumed.
        let client = server.clients.get(&client_id).unwrap();
        assert!(
            client
                .inventory
                .inventory_slots
                .iter()
                .chain(client.inventory.actionbar_slots.iter())
                .any(|slot| slot
                    .as_ref()
                    .is_some_and(|stack| stack.item_id.as_ref() == WORKBENCH_T1_ID)),
            "workbench should still be in inventory"
        );
    }

    #[test]
    fn overlapping_placement_is_rejected() {
        let mut server = make_server();
        let client_id = connect_test_client(&mut server);
        give_one(&mut server, client_id, WORKBENCH_T1_ID);
        give_one(&mut server, client_id, CRUDE_FURNACE_ID);

        server.apply_place_deployable_command(
            client_id,
            PlaceDeployableCommand {
                item_id: intern_item_id(WORKBENCH_T1_ID),
                position: Vec3Net::new(1.2, 0.0, 0.0),
                yaw: 0.0,
            },
        );
        let envelopes = server.apply_place_deployable_command(
            client_id,
            PlaceDeployableCommand {
                item_id: intern_item_id(CRUDE_FURNACE_ID),
                position: Vec3Net::new(1.2, 0.0, 0.0),
                yaw: 0.0,
            },
        );
        assert!(envelopes.iter().any(|e| matches!(&e.message, ServerMessage::Toast(t) if matches!(t.kind, ToastKind::Warning))));
        assert_eq!(
            server.deployed_entities.len(),
            1,
            "second placement must fail"
        );
    }

    fn place_workbench(server: &mut GameServer, client_id: ClientId) -> DeployedEntityId {
        server.apply_place_deployable_command(
            client_id,
            PlaceDeployableCommand {
                item_id: intern_item_id(WORKBENCH_T1_ID),
                position: Vec3Net::new(1.5, 0.0, 0.0),
                yaw: 0.0,
            },
        );
        *server
            .deployed_entities
            .keys()
            .next()
            .expect("workbench placed")
    }

    fn equip_pickaxe(server: &mut GameServer, client_id: ClientId) {
        use crate::{
            items::BASIC_PICKAXE_ID, protocol::ItemStack, server::inventory::add_stack_to_inventory,
        };
        let client = server.clients.get_mut(&client_id).unwrap();
        // Drop the pickaxe straight into the active actionbar slot so
        // the tool-profile lookup in the damage handler picks it up
        // without a manual move.
        client.inventory.actionbar_slots[0] = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));
        // Re-merge so any limit logic runs.
        let leftover =
            add_stack_to_inventory(&mut client.inventory, ItemStack::new(BASIC_PICKAXE_ID, 0));
        assert!(leftover.is_none());
    }

    fn equip_hatchet(server: &mut GameServer, client_id: ClientId) {
        use crate::{
            items::BASIC_HATCHET_ID, protocol::ItemStack, server::inventory::add_stack_to_inventory,
        };
        let client = server.clients.get_mut(&client_id).unwrap();
        client.inventory.actionbar_slots[0] = Some(ItemStack::new(BASIC_HATCHET_ID, 1));
        let leftover =
            add_stack_to_inventory(&mut client.inventory, ItemStack::new(BASIC_HATCHET_ID, 0));
        assert!(leftover.is_none());
    }

    fn place_furnace(server: &mut GameServer, client_id: ClientId) -> DeployedEntityId {
        server.apply_place_deployable_command(
            client_id,
            PlaceDeployableCommand {
                item_id: intern_item_id(CRUDE_FURNACE_ID),
                position: Vec3Net::new(1.5, 0.0, 0.0),
                yaw: 0.0,
            },
        );
        *server
            .deployed_entities
            .keys()
            .next()
            .expect("furnace placed")
    }

    #[test]
    fn damage_reduces_health_and_respects_tool_cooldown() {
        use crate::protocol::DamageDeployableCommand;
        let mut server = make_server();
        let client_id = connect_test_client(&mut server);
        give_one(&mut server, client_id, WORKBENCH_T1_ID);
        let id = place_workbench(&mut server, client_id);
        equip_pickaxe(&mut server, client_id);

        let start_hp = server.deployed_entities[&id].health;
        server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
        let after_first = server.deployed_entities[&id].health;
        assert!(after_first < start_hp, "first hit reduces health");

        // Same tick → cooldown blocks the second hit.
        server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
        let after_second = server.deployed_entities[&id].health;
        assert_eq!(
            after_second, after_first,
            "cooldown must block back-to-back damage"
        );
    }

    #[test]
    fn damage_destroys_at_zero_health() {
        use crate::protocol::DamageDeployableCommand;
        let mut server = make_server();
        let client_id = connect_test_client(&mut server);
        give_one(&mut server, client_id, WORKBENCH_T1_ID);
        let id = place_workbench(&mut server, client_id);
        equip_pickaxe(&mut server, client_id);

        // Force-empty the structure's HP and check that one more hit
        // despawns it.
        if let Some(entity) = server.deployed_entities.get_mut(&id) {
            entity.health = 1;
        }
        server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
        assert!(
            !server.deployed_entities.contains_key(&id),
            "deployable should be removed when HP reaches 0"
        );
    }

    #[test]
    fn matched_tool_outdamages_mismatched_tool() {
        use crate::protocol::DamageDeployableCommand;
        // Workbench (wood) vs hatchet should deal 3× the per-hit damage
        // of pickaxe vs the same workbench (150% vs 50%).
        let mut axe_server = make_server();
        let axe_client = connect_test_client(&mut axe_server);
        give_one(&mut axe_server, axe_client, WORKBENCH_T1_ID);
        let axe_target = place_workbench(&mut axe_server, axe_client);
        equip_hatchet(&mut axe_server, axe_client);

        let start_hp = axe_server.deployed_entities[&axe_target].health;
        axe_server.apply_damage_deployable_command(
            axe_client,
            DamageDeployableCommand { id: axe_target },
        );
        let axe_damage = start_hp - axe_server.deployed_entities[&axe_target].health;

        let mut pick_server = make_server();
        let pick_client = connect_test_client(&mut pick_server);
        give_one(&mut pick_server, pick_client, WORKBENCH_T1_ID);
        let pick_target = place_workbench(&mut pick_server, pick_client);
        equip_pickaxe(&mut pick_server, pick_client);

        pick_server.apply_damage_deployable_command(
            pick_client,
            DamageDeployableCommand { id: pick_target },
        );
        let pick_damage = start_hp - pick_server.deployed_entities[&pick_target].health;

        assert_eq!(
            axe_damage,
            pick_damage * 3,
            "hatchet (150% on wood) should out-damage pickaxe (50% on wood) by 3×"
        );
    }

    #[test]
    fn pickaxe_outdamages_hatchet_on_furnace() {
        use crate::protocol::DamageDeployableCommand;
        let mut pick_server = make_server();
        let pick_client = connect_test_client(&mut pick_server);
        give_one(&mut pick_server, pick_client, CRUDE_FURNACE_ID);
        let pick_target = place_furnace(&mut pick_server, pick_client);
        equip_pickaxe(&mut pick_server, pick_client);

        let start_hp = pick_server.deployed_entities[&pick_target].health;
        pick_server.apply_damage_deployable_command(
            pick_client,
            DamageDeployableCommand { id: pick_target },
        );
        let pick_damage = start_hp - pick_server.deployed_entities[&pick_target].health;

        let mut axe_server = make_server();
        let axe_client = connect_test_client(&mut axe_server);
        give_one(&mut axe_server, axe_client, CRUDE_FURNACE_ID);
        let axe_target = place_furnace(&mut axe_server, axe_client);
        equip_hatchet(&mut axe_server, axe_client);

        axe_server.apply_damage_deployable_command(
            axe_client,
            DamageDeployableCommand { id: axe_target },
        );
        let axe_damage = start_hp - axe_server.deployed_entities[&axe_target].health;

        assert_eq!(
            pick_damage,
            axe_damage * 3,
            "pickaxe (150% on stone) should out-damage hatchet (50% on stone) by 3×"
        );
    }

    #[test]
    fn bare_hands_cannot_damage_deployables() {
        use crate::protocol::DamageDeployableCommand;
        let mut server = make_server();
        let client_id = connect_test_client(&mut server);
        give_one(&mut server, client_id, WORKBENCH_T1_ID);
        let id = place_workbench(&mut server, client_id);

        let start_hp = server.deployed_entities[&id].health;
        server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id });
        let after = server.deployed_entities[&id].health;
        assert_eq!(after, start_hp, "no tool → no damage");
    }

    #[test]
    fn station_in_range_matches_only_close_workbench() {
        let mut server = make_server();
        let client_id = connect_test_client(&mut server);
        give_one(&mut server, client_id, WORKBENCH_T1_ID);

        // Place a workbench right next to the player (spawn is near origin).
        server.apply_place_deployable_command(
            client_id,
            PlaceDeployableCommand {
                item_id: intern_item_id(WORKBENCH_T1_ID),
                position: Vec3Net::new(1.5, 0.0, 0.0),
                yaw: 0.0,
            },
        );
        assert!(server.station_in_range(client_id, RecipeStation::Workbench { min_tier: 1 }));
        // A higher-tier workbench requirement is not satisfied by a T1.
        assert!(!server.station_in_range(client_id, RecipeStation::Workbench { min_tier: 2 }));
    }
}
