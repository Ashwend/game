use std::collections::HashMap;

use crate::{
    controller::PlayerController,
    items::{can_pick_up, normalize_stack, stack_limit},
    protocol::{
        ACTIONBAR_SLOT_COUNT, ChatMessage, ClientId, ClientMessage, DroppedItemId,
        DroppedWorldItem, InventoryCommand, ItemStack, PlayerInventoryState, ResourceNodeId,
        ResourceNodeState, ServerMessage, SteamId, Vec3Net, sanitize_chat,
    },
    save::WorldSave,
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
    settings: ServerSettings,
    clients: HashMap<ClientId, ServerClient>,
    steam_to_client: HashMap<SteamId, ClientId>,
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
        let dropped_item_physics = DroppedItemPhysics::new(&world);
        let resource_nodes = resource_nodes::initial_resource_nodes(&world);

        Self {
            tick: save.state.last_authoritative_tick,
            save,
            world,
            settings,
            clients: HashMap::new(),
            steam_to_client: HashMap::new(),
            dropped_items: HashMap::new(),
            dropped_item_physics,
            resource_nodes,
            next_dropped_item_id: 1,
            next_client_id: 1,
        }
    }

    pub fn world_save(&self) -> WorldSave {
        let mut save = self.save.clone();
        save.state.last_authoritative_tick = self.tick;
        save
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
            ClientMessage::Inventory(command) => {
                self.apply_inventory_command(client_id, command);
                Vec::new()
            }
            ClientMessage::Gather(command) => {
                self.apply_gather_command(client_id, command);
                Vec::new()
            }
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

        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::Snapshot(self.snapshot()),
        });
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

    fn apply_inventory_command(&mut self, client_id: ClientId, command: InventoryCommand) {
        match command {
            InventoryCommand::Move { from, to, quantity } => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    move_stack(&mut client.inventory, from, to, quantity);
                }
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
                    return;
                };
                self.spawn_dropped_item(stack, position, velocity, yaw);
            }
            InventoryCommand::PickUp { dropped_item_id } => {
                self.pick_up_dropped_item(client_id, dropped_item_id);
            }
            InventoryCommand::SelectActionbarSlot { slot } => {
                if slot < ACTIONBAR_SLOT_COUNT
                    && let Some(client) = self.clients.get_mut(&client_id)
                {
                    client.inventory.active_actionbar_slot = slot;
                }
            }
            InventoryCommand::SelectActionbarOffset { offset } => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.inventory.active_actionbar_slot =
                        offset_actionbar_slot(client.inventory.active_actionbar_slot, offset);
                }
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

    fn pick_up_dropped_item(&mut self, client_id: ClientId, dropped_item_id: DroppedItemId) {
        let Some(item) = self
            .dropped_items
            .get(&dropped_item_id)
            .map(|body| body.item.clone())
        else {
            return;
        };
        let Some(client) = self.clients.get(&client_id) else {
            return;
        };
        if !can_pick_up(
            player_eye_position(client.controller.position),
            client.controller.yaw,
            client.controller.pitch,
            &item,
        ) {
            return;
        }

        let Some(client) = self.clients.get_mut(&client_id) else {
            return;
        };
        if add_stack_to_inventory(&mut client.inventory, item.stack.clone()).is_none()
            && let Some(body) = self.dropped_items.remove(&dropped_item_id)
        {
            self.dropped_item_physics.remove_body(body.body_handle);
        }
    }

    fn merge_nearby_dropped_items(&mut self) -> Vec<(String, u16)> {
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
    ) -> Option<(String, u16)> {
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

#[derive(Debug)]
struct ServerClient {
    client_id: ClientId,
    steam_id: SteamId,
    name: String,
    controller: PlayerController,
    inventory: PlayerInventoryState,
    is_admin: bool,
    last_seen_tick: u64,
    next_gather_tick: u64,
}

#[cfg(test)]
mod tests;
