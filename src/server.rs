use std::collections::HashMap;

use crate::{
    controller::{BlockGrid, PlayerController},
    items::{ItemId, can_pick_up, item_definition, normalize_stack, stack_limit},
    protocol::{
        ACTIONBAR_SLOT_COUNT, ChatMessage, ClientId, ClientMessage, DroppedItemId,
        DroppedWorldItem, InventoryCommand, ItemStack, PlayerInventoryState, ResourceNodeId,
        ResourceNodeState, ServerMessage, SteamId, ToastKind, ToastMessage, Vec3Net, sanitize_chat,
    },
    save::{PersistedPlayer, WorldSave, WorldStateSave},
    steam::AuthMode,
    world::WorldData,
};

const CLIENT_STALE_TIMEOUT_TICKS: u64 = 20 * 10;

mod connection;
mod dropped_items;
mod inventory;
mod movement;
mod resource_nodes;

use self::{
    dropped_items::{
        DROPPED_ITEM_MERGE_INTERVAL_TICKS, DroppedItemBody, DroppedItemPhysics,
        nearby_dropped_item_pairs, yaw_rotation,
    },
    inventory::{add_stack_to_inventory, move_stack, offset_actionbar_slot, remove_stack},
    movement::{accept_client_movement, drop_position, drop_velocity, player_eye_position},
};

#[derive(Debug, Clone)]
pub struct ServerSettings {
    pub auth_mode: AuthMode,
    pub singleplayer_host: Option<SteamId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryTarget {
    Client(ClientId),
    Broadcast,
    /// Send to every connected client except the named one. Used for
    /// "echo to peers" payloads (e.g. impact effects) where the originating
    /// client already produced the effect locally via prediction and a
    /// second copy from the server would double-trigger it.
    BroadcastExcept(ClientId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerEnvelope {
    pub target: DeliveryTarget,
    pub message: ServerMessage,
}

#[derive(Debug)]
pub struct GameServer {
    save: WorldSave,
    world: WorldData,
    /// Spatial index over `world.blocks`. Built once at construction. Movement
    /// is currently client-authoritative so the server doesn't simulate, but
    /// the grid is here for the next time a server-side collision check (e.g.
    /// drop validation, future server-authoritative movement) is wired in.
    #[allow(dead_code)]
    world_grid: BlockGrid,
    settings: ServerSettings,
    clients: HashMap<ClientId, ServerClient>,
    steam_to_client: HashMap<SteamId, ClientId>,
    /// Players who have ever been seen on this server, keyed by Steam ID. A
    /// disconnect or shutdown writes back into this map so a returning player
    /// picks up their inventory, position, and admin status.
    persisted_players: HashMap<SteamId, PersistedPlayer>,
    dropped_items: HashMap<DroppedItemId, DroppedItemBody>,
    dropped_item_physics: DroppedItemPhysics,
    resource_nodes: HashMap<ResourceNodeId, ResourceNodeState>,
    next_dropped_item_id: DroppedItemId,
    next_client_id: ClientId,
    tick: u64,
}

impl GameServer {
    pub fn new(mut save: WorldSave, settings: ServerSettings) -> Self {
        if let Some(host) = settings.singleplayer_host
            && !save.admins.contains(&host)
        {
            save.admins.push(host);
        }
        let world = save.map.world_data();
        let world_grid = BlockGrid::build(&world);
        let mut dropped_item_physics = DroppedItemPhysics::new(&world);

        // Resource nodes: trust the saved state once a world has ever been
        // hosted (so harvested resources don't respawn). For brand-new worlds
        // the save has `None` and we seed from the world definition.
        let resource_nodes = match save.state.resource_nodes.take() {
            Some(saved) => saved.into_iter().map(|node| (node.id, node)).collect(),
            None => resource_nodes::initial_resource_nodes(&world),
        };

        let mut dropped_items = HashMap::new();
        for item in std::mem::take(&mut save.state.dropped_items) {
            let physics_body =
                dropped_item_physics.spawn_body(item.position, Vec3Net::ZERO, item.yaw);
            dropped_items.insert(
                item.id,
                DroppedItemBody {
                    item,
                    body_handle: physics_body.body_handle,
                },
            );
        }

        let persisted_players = std::mem::take(&mut save.state.players)
            .into_iter()
            .map(|player| (player.steam_id, player))
            .collect();

        let next_dropped_item_id = save.state.next_dropped_item_id.max(1);
        let next_client_id = save.state.next_client_id.max(1);

        Self {
            tick: save.state.last_authoritative_tick,
            save,
            world,
            world_grid,
            settings,
            clients: HashMap::new(),
            steam_to_client: HashMap::new(),
            persisted_players,
            dropped_items,
            dropped_item_physics,
            resource_nodes,
            next_dropped_item_id,
            next_client_id,
        }
    }

    pub fn world_save(&self) -> WorldSave {
        let mut save = self.save.clone();
        let mut persisted = self.persisted_players.clone();
        // Capture any currently connected players' live state before writing.
        for client in self.clients.values() {
            persisted.insert(client.steam_id, persisted_player_from(client));
        }
        let mut players = persisted.into_values().collect::<Vec<_>>();
        players.sort_by_key(|player| player.steam_id);

        let mut dropped_items = self
            .dropped_items
            .values()
            .map(|body| body.item.clone())
            .collect::<Vec<_>>();
        dropped_items.sort_by_key(|item| item.id);

        let mut resource_nodes = self.resource_nodes.values().cloned().collect::<Vec<_>>();
        resource_nodes.sort_by_key(|node| node.id);

        save.state = WorldStateSave {
            last_authoritative_tick: self.tick,
            players,
            dropped_items,
            resource_nodes: Some(resource_nodes),
            next_dropped_item_id: self.next_dropped_item_id,
            next_client_id: self.next_client_id,
        };
        save
    }

    pub(super) fn take_persisted_player(&mut self, steam_id: SteamId) -> Option<PersistedPlayer> {
        self.persisted_players.remove(&steam_id)
    }

    pub(super) fn remember_player(&mut self, player: PersistedPlayer) {
        self.persisted_players.insert(player.steam_id, player);
    }

    pub fn receive(&mut self, client_id: ClientId, message: ClientMessage) -> Vec<ServerEnvelope> {
        self.mark_client_seen(client_id);

        match message {
            ClientMessage::Auth { .. } => vec![ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::AuthRejected {
                    reason: "client is already authenticated".to_owned(),
                },
            }],
            ClientMessage::Movement(movement) => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    accept_client_movement(&mut client.controller, movement);
                }
                Vec::new()
            }
            ClientMessage::Chat { text } => sanitize_chat(&text)
                .and_then(|text| {
                    self.clients.get(&client_id).map(|client| ServerEnvelope {
                        target: DeliveryTarget::Broadcast,
                        message: ServerMessage::Chat(ChatMessage {
                            from: client.name.clone(),
                            text,
                        }),
                    })
                })
                .into_iter()
                .collect(),
            ClientMessage::Inventory(command) => self.apply_inventory_command(client_id, command),
            ClientMessage::Gather(command) => self.apply_gather_command(client_id, command),
            ClientMessage::Heartbeat => Vec::new(),
            ClientMessage::Disconnect => self.disconnect(client_id),
        }
    }

    pub fn announce(&self, text: impl AsRef<str>) -> Vec<ServerEnvelope> {
        sanitize_chat(text.as_ref())
            .map(|text| ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::Chat(ChatMessage {
                    from: "Server".to_owned(),
                    text,
                }),
            })
            .into_iter()
            .collect()
    }

    pub fn kick_all(&mut self, reason: impl Into<String>) -> Vec<ServerEnvelope> {
        let reason = reason.into();
        let client_ids = self.clients.keys().copied().collect::<Vec<_>>();
        let mut envelopes = client_ids
            .iter()
            .copied()
            .map(|client_id| ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::Kicked {
                    reason: reason.clone(),
                },
            })
            .collect::<Vec<_>>();

        for client_id in client_ids {
            envelopes.extend(self.disconnect(client_id));
        }

        envelopes
    }

    pub fn tick(&mut self, delta_seconds: f32) -> Vec<ServerEnvelope> {
        self.tick += 1;
        self.save.state.last_authoritative_tick = self.tick;
        self.dropped_item_physics
            .step(delta_seconds, &mut self.dropped_items);

        let mut envelopes = self.disconnect_stale_clients();
        if self.tick.is_multiple_of(DROPPED_ITEM_MERGE_INTERVAL_TICKS) {
            envelopes.extend(self.merge_nearby_dropped_items().into_iter().map(
                |(item_id, quantity)| ServerEnvelope {
                    target: DeliveryTarget::Broadcast,
                    message: ServerMessage::ItemMerged { item_id, quantity },
                },
            ));
        }

        // Per-client snapshots: each client gets a copy where only their own
        // player carries the inventory payload. Saves bandwidth and keeps
        // hotbar contents private without needing a separate inventory
        // message channel.
        let client_ids = self.clients.keys().copied().collect::<Vec<_>>();
        for client_id in client_ids {
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::Snapshot(self.snapshot_for(client_id)),
            });
        }
        envelopes
    }

    fn mark_client_seen(&mut self, client_id: ClientId) {
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.last_seen_tick = self.tick;
        }
    }

    fn disconnect_stale_clients(&mut self) -> Vec<ServerEnvelope> {
        let stale_client_ids = self
            .clients
            .values()
            .filter(|client| {
                self.tick.saturating_sub(client.last_seen_tick) > CLIENT_STALE_TIMEOUT_TICKS
            })
            .map(|client| client.client_id)
            .collect::<Vec<_>>();

        stale_client_ids
            .into_iter()
            .flat_map(|client_id| self.disconnect(client_id))
            .collect()
    }

    fn apply_inventory_command(
        &mut self,
        client_id: ClientId,
        command: InventoryCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            InventoryCommand::Move { from, to, quantity } => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    move_stack(&mut client.inventory, from, to, quantity);
                }
                Vec::new()
            }
            InventoryCommand::Drop { from, quantity } => {
                let Some((stack, position, velocity, yaw)) =
                    self.clients.get_mut(&client_id).and_then(|client| {
                        remove_stack(&mut client.inventory, from, quantity).map(|stack| {
                            (
                                stack,
                                drop_position(&client.controller),
                                drop_velocity(&client.controller),
                                client.controller.yaw,
                            )
                        })
                    })
                else {
                    return Vec::new();
                };
                self.spawn_dropped_item(stack, position, velocity, yaw);
                Vec::new()
            }
            InventoryCommand::PickUp { dropped_item_id } => {
                self.pick_up_dropped_item(client_id, dropped_item_id)
            }
            InventoryCommand::SelectActionbarSlot { slot } => {
                if slot < ACTIONBAR_SLOT_COUNT
                    && let Some(client) = self.clients.get_mut(&client_id)
                {
                    client.inventory.active_actionbar_slot = slot;
                }
                Vec::new()
            }
            InventoryCommand::SelectActionbarOffset { offset } => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.inventory.active_actionbar_slot =
                        offset_actionbar_slot(client.inventory.active_actionbar_slot, offset);
                }
                Vec::new()
            }
        }
    }

    fn spawn_dropped_item(
        &mut self,
        stack: ItemStack,
        position: Vec3Net,
        velocity: Vec3Net,
        yaw: f32,
    ) {
        let Some(stack) = normalize_stack(&stack) else {
            return;
        };
        let id = self.next_dropped_item_id;
        self.next_dropped_item_id += 1;
        let physics_body = self
            .dropped_item_physics
            .spawn_body(position, velocity, yaw);
        self.dropped_items.insert(
            id,
            DroppedItemBody {
                item: DroppedWorldItem {
                    id,
                    stack,
                    position,
                    yaw,
                    rotation: yaw_rotation(yaw),
                },
                body_handle: physics_body.body_handle,
            },
        );
    }

    fn pick_up_dropped_item(
        &mut self,
        client_id: ClientId,
        dropped_item_id: DroppedItemId,
    ) -> Vec<ServerEnvelope> {
        let Some(item) = self
            .dropped_items
            .get(&dropped_item_id)
            .map(|body| body.item.clone())
        else {
            return Vec::new();
        };
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !can_pick_up(
            player_eye_position(client.controller.position),
            client.controller.yaw,
            client.controller.pitch,
            &item,
        ) {
            return Vec::new();
        }

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let requested = item.stack.quantity;
        let remainder = add_stack_to_inventory(&mut client.inventory, item.stack.clone());
        let accepted = match &remainder {
            Some(rem) => requested.saturating_sub(rem.quantity),
            None => requested,
        };
        if remainder.is_none()
            && let Some(body) = self.dropped_items.remove(&dropped_item_id)
        {
            self.dropped_item_physics.remove_body(body.body_handle);
        }
        if accepted == 0 {
            return Vec::new();
        }
        item_acquired_toast_envelopes(client_id, &item.stack.item_id, accepted)
    }

    fn merge_nearby_dropped_items(&mut self) -> Vec<(crate::items::ItemId, u16)> {
        // Returns the interned `ItemId` (not a fresh `String`) so the
        // resulting `ServerMessage::ItemMerged` doesn't allocate per merge.
        let mut merges = Vec::new();
        for (first_id, second_id) in nearby_dropped_item_pairs(&self.dropped_items) {
            if let Some(merge) = self.merge_dropped_item_pair(first_id, second_id) {
                merges.push(merge);
            }
        }
        merges
    }

    fn merge_dropped_item_pair(
        &mut self,
        first_id: DroppedItemId,
        second_id: DroppedItemId,
    ) -> Option<(crate::items::ItemId, u16)> {
        let (target_id, source_id) = self.merge_target_and_source(first_id, second_id)?;
        let mut source = self.dropped_items.remove(&source_id)?;
        let Some(target) = self.dropped_items.get_mut(&target_id) else {
            self.dropped_items.insert(source_id, source);
            return None;
        };
        let Some(limit) = stack_limit(&target.item.stack.item_id) else {
            self.dropped_items.insert(source_id, source);
            return None;
        };
        let room = limit.saturating_sub(target.item.stack.quantity);
        let moved = room.min(source.item.stack.quantity);
        if moved == 0 {
            self.dropped_items.insert(source_id, source);
            return None;
        }

        target.item.stack.quantity += moved;
        source.item.stack.quantity -= moved;
        let item_id = target.item.stack.item_id.clone();
        if source.item.stack.quantity == 0 {
            self.dropped_item_physics.remove_body(source.body_handle);
        } else {
            self.dropped_items.insert(source_id, source);
        }

        Some((item_id, moved))
    }

    fn merge_target_and_source(
        &self,
        first_id: DroppedItemId,
        second_id: DroppedItemId,
    ) -> Option<(DroppedItemId, DroppedItemId)> {
        let first = self.dropped_items.get(&first_id)?;
        let second = self.dropped_items.get(&second_id)?;
        if first.item.stack.item_id != second.item.stack.item_id {
            return None;
        }

        let limit = stack_limit(&first.item.stack.item_id)?;
        let first_room = limit.saturating_sub(first.item.stack.quantity);
        let second_room = limit.saturating_sub(second.item.stack.quantity);
        match (first_room > 0, second_room > 0) {
            (false, false) => None,
            (true, false) => Some((first_id, second_id)),
            (false, true) => Some((second_id, first_id)),
            (true, true) if first.item.stack.quantity >= second.item.stack.quantity => {
                Some((first_id, second_id))
            }
            (true, true) => Some((second_id, first_id)),
        }
    }
}

/// Builds the "you just acquired N items" toast envelope used by both the
/// resource gathering path and the dropped-item pickup path. Lives in
/// `server.rs` so submodules can share it without cross-module reach-around.
pub(super) fn item_acquired_toast_envelopes(
    client_id: ClientId,
    item_id: &ItemId,
    quantity: u16,
) -> Vec<ServerEnvelope> {
    if quantity == 0 {
        return Vec::new();
    }
    let Some(definition) = item_definition(item_id) else {
        return Vec::new();
    };
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(
            ToastKind::Success,
            format!("+{quantity} {}", definition.name),
        )),
    }]
}

#[derive(Debug)]
pub(super) struct ServerClient {
    pub(super) client_id: ClientId,
    pub(super) steam_id: SteamId,
    pub(super) name: String,
    pub(super) controller: PlayerController,
    pub(super) inventory: PlayerInventoryState,
    pub(super) is_admin: bool,
    pub(super) last_seen_tick: u64,
    pub(super) next_gather_tick: u64,
}

pub(super) fn persisted_player_from(client: &ServerClient) -> PersistedPlayer {
    PersistedPlayer {
        steam_id: client.steam_id,
        name: client.name.clone(),
        position: client.controller.position,
        velocity: client.controller.velocity,
        yaw: client.controller.yaw,
        pitch: client.controller.pitch,
        health: client.controller.health,
        grounded: client.controller.grounded,
        last_processed_input: client.controller.last_processed_input,
        is_admin: client.is_admin,
        inventory: client.inventory.clone(),
    }
}

#[cfg(test)]
mod tests;
