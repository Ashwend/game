use std::{
    collections::HashMap,
    f32::consts::{PI, TAU},
};

use anyhow::{Result, bail};

use crate::{
    controller::{MAX_LOOK_PITCH, PlayerController},
    items::{
        TEST_BANDAGE_ID, TEST_ORE_ID, TEST_RELIC_ID, can_pick_up, normalize_stack, stack_limit,
    },
    protocol::{
        ACTIONBAR_SLOT_COUNT, ChatMessage, ClientId, ClientMessage, DroppedItemId,
        DroppedWorldItem, INVENTORY_SLOT_COUNT, InventoryCommand, ItemContainer, ItemContainerSlot,
        ItemStack, PROTOCOL_VERSION, PlayerEvent, PlayerInventoryState, PlayerMovement,
        PlayerState, ServerMessage, SteamId, Vec3Net, WorldSnapshot, sanitize_chat,
    },
    save::WorldSave,
    steam::{AuthMode, verify_auth_ticket},
    world::WorldData,
};

const SERVER_EYE_HEIGHT: f32 = 1.62;
const DROP_FORWARD_DISTANCE: f32 = 1.45;
const DROPPED_ITEM_GROUND_Y: f32 = 0.14;

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
    dropped_items: HashMap<DroppedItemId, DroppedWorldItem>,
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

        Self {
            tick: save.state.last_authoritative_tick,
            save,
            world,
            settings,
            clients: HashMap::new(),
            steam_to_client: HashMap::new(),
            dropped_items: HashMap::new(),
            next_dropped_item_id: 1,
            next_client_id: 1,
        }
    }

    pub fn world_save(&self) -> WorldSave {
        let mut save = self.save.clone();
        save.state.last_authoritative_tick = self.tick;
        save
    }

    pub fn connect(
        &mut self,
        protocol_version: u32,
        steam_id: SteamId,
        display_name: String,
        token: String,
    ) -> Result<(ClientId, Vec<ServerEnvelope>)> {
        if protocol_version != PROTOCOL_VERSION {
            bail!("protocol mismatch: client {protocol_version}, server {PROTOCOL_VERSION}");
        }

        verify_auth_ticket(self.settings.auth_mode, steam_id, &token)?;

        if self.steam_to_client.contains_key(&steam_id) {
            bail!("this Steam user is already connected");
        }

        let client_id = self.next_client_id;
        self.next_client_id += 1;

        let is_admin = self.is_admin(steam_id);
        let name = clean_player_name(&display_name, client_id);
        let client = ServerClient {
            client_id,
            steam_id,
            name: name.clone(),
            controller: PlayerController::spawn(),
            inventory: starting_inventory(),
            is_admin,
        };

        self.clients.insert(client_id, client);
        self.steam_to_client.insert(steam_id, client_id);

        let snapshot = self.snapshot();
        Ok((
            client_id,
            vec![
                ServerEnvelope {
                    target: DeliveryTarget::Client(client_id),
                    message: ServerMessage::Welcome {
                        client_id,
                        map: self.save.map.clone(),
                        world: self.world.clone(),
                        is_admin,
                        snapshot,
                    },
                },
                ServerEnvelope {
                    target: DeliveryTarget::Broadcast,
                    message: ServerMessage::PlayerEvent(PlayerEvent::Joined { client_id, name }),
                },
            ],
        ))
    }

    pub fn receive(&mut self, client_id: ClientId, message: ClientMessage) -> Vec<ServerEnvelope> {
        match message {
            ClientMessage::Auth { .. } => vec![ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::AuthRejected {
                    reason: "client is already authenticated".to_owned(),
                },
            }],
            ClientMessage::Movement(movement) => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    apply_client_movement(&mut client.controller, movement);
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
            ClientMessage::Heartbeat => Vec::new(),
            ClientMessage::Disconnect => self.disconnect(client_id),
        }
    }

    pub fn disconnect(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.remove(&client_id) else {
            return Vec::new();
        };

        self.steam_to_client.remove(&client.steam_id);
        vec![ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::PlayerEvent(PlayerEvent::Left {
                client_id,
                name: client.name,
            }),
        }]
    }

    pub fn tick(&mut self, _delta_seconds: f32) -> Vec<ServerEnvelope> {
        self.tick += 1;
        self.save.state.last_authoritative_tick = self.tick;

        vec![ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::Snapshot(self.snapshot()),
        }]
    }

    pub fn snapshot(&self) -> WorldSnapshot {
        let mut players = self
            .clients
            .values()
            .map(|client| PlayerState {
                client_id: client.client_id,
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
            })
            .collect::<Vec<_>>();
        players.sort_by_key(|player| player.client_id);

        let mut dropped_items = self.dropped_items.values().cloned().collect::<Vec<_>>();
        dropped_items.sort_by_key(|item| item.id);

        WorldSnapshot {
            tick: self.tick,
            players,
            dropped_items,
        }
    }

    fn is_admin(&self, steam_id: SteamId) -> bool {
        self.settings.singleplayer_host == Some(steam_id) || self.save.admins.contains(&steam_id)
    }

    fn apply_inventory_command(&mut self, client_id: ClientId, command: InventoryCommand) {
        match command {
            InventoryCommand::Move { from, to, quantity } => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    move_stack(&mut client.inventory, from, to, quantity);
                }
            }
            InventoryCommand::Drop { from, quantity } => {
                let Some((stack, position, yaw)) =
                    self.clients.get_mut(&client_id).and_then(|client| {
                        remove_stack(&mut client.inventory, from, quantity).map(|stack| {
                            (
                                stack,
                                drop_position(&client.controller),
                                client.controller.yaw,
                            )
                        })
                    })
                else {
                    return;
                };
                self.spawn_dropped_item(stack, position, yaw);
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

    fn spawn_dropped_item(&mut self, stack: ItemStack, position: Vec3Net, yaw: f32) {
        let Some(stack) = normalize_stack(&stack) else {
            return;
        };
        let id = self.next_dropped_item_id;
        self.next_dropped_item_id += 1;
        self.dropped_items.insert(
            id,
            DroppedWorldItem {
                id,
                stack,
                position,
                yaw,
            },
        );
    }

    fn pick_up_dropped_item(&mut self, client_id: ClientId, dropped_item_id: DroppedItemId) {
        let Some(item) = self.dropped_items.get(&dropped_item_id).cloned() else {
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
        if add_stack_to_inventory(&mut client.inventory, item.stack.clone()).is_none() {
            self.dropped_items.remove(&dropped_item_id);
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
}

fn starting_inventory() -> PlayerInventoryState {
    let mut inventory = PlayerInventoryState::empty();
    inventory.inventory_slots[0] = Some(ItemStack::new(TEST_ORE_ID, 12));
    inventory.inventory_slots[1] = Some(ItemStack::new(TEST_BANDAGE_ID, 5));
    inventory.inventory_slots[2] = Some(ItemStack::new(TEST_RELIC_ID, 1));
    inventory
}

fn move_stack(
    inventory: &mut PlayerInventoryState,
    from: ItemContainerSlot,
    to: ItemContainerSlot,
    quantity: Option<u16>,
) {
    if from == to || !slot_exists(inventory, from) || !slot_exists(inventory, to) {
        return;
    }

    let Some((moving, removed_all)) = remove_stack_for_move(inventory, from, quantity) else {
        return;
    };
    let remainder = insert_stack_at(inventory, to, moving, removed_all);
    if let Some(remainder) = remainder {
        restore_stack(inventory, from, remainder);
    }
}

fn remove_stack_for_move(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
    quantity: Option<u16>,
) -> Option<(ItemStack, bool)> {
    let source = slot_mut(inventory, slot)?;
    let current = source.as_mut()?;
    let amount = quantity
        .unwrap_or(current.quantity)
        .clamp(1, current.quantity);
    let removed_all = amount == current.quantity;
    let item_id = current.item_id.clone();
    current.quantity -= amount;
    if current.quantity == 0 {
        *source = None;
    }
    Some((ItemStack::new(item_id, amount), removed_all))
}

fn remove_stack(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
    quantity: Option<u16>,
) -> Option<ItemStack> {
    let source = slot_mut(inventory, slot)?;
    let current = source.as_mut()?;
    let amount = quantity
        .unwrap_or(current.quantity)
        .clamp(1, current.quantity);
    let item_id = current.item_id.clone();
    current.quantity -= amount;
    if current.quantity == 0 {
        *source = None;
    }
    Some(ItemStack::new(item_id, amount))
}

fn insert_stack_at(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
    mut moving: ItemStack,
    allow_swap: bool,
) -> Option<ItemStack> {
    moving = normalize_stack(&moving)?;
    let target = slot_mut(inventory, slot)?;
    match target {
        None => {
            *target = Some(moving);
            None
        }
        Some(existing) if existing.item_id == moving.item_id => {
            let limit = stack_limit(&existing.item_id).unwrap_or(1);
            let room = limit.saturating_sub(existing.quantity);
            let moved = room.min(moving.quantity);
            existing.quantity += moved;
            moving.quantity -= moved;
            (moving.quantity > 0).then_some(moving)
        }
        Some(existing) if allow_swap => {
            let displaced = std::mem::replace(existing, moving);
            Some(displaced)
        }
        Some(_) => Some(moving),
    }
}

fn restore_stack(inventory: &mut PlayerInventoryState, slot: ItemContainerSlot, stack: ItemStack) {
    let Some(target) = slot_mut(inventory, slot) else {
        return;
    };
    match target {
        Some(existing) if existing.item_id == stack.item_id => {
            let limit = stack_limit(&existing.item_id).unwrap_or(1);
            existing.quantity = existing.quantity.saturating_add(stack.quantity).min(limit);
        }
        None => {
            *target = Some(stack);
        }
        Some(_) => {}
    }
}

fn add_stack_to_inventory(
    inventory: &mut PlayerInventoryState,
    stack: ItemStack,
) -> Option<ItemStack> {
    let mut remaining = normalize_stack(&stack)?;

    for index in 0..inventory.inventory_slots.len() {
        let slot = ItemContainerSlot::inventory(index);
        if inventory.inventory_slots[index]
            .as_ref()
            .is_some_and(|existing| existing.item_id == remaining.item_id)
        {
            remaining = match insert_stack_at(inventory, slot, remaining, false) {
                Some(remaining) => remaining,
                None => return None,
            };
        }
    }

    for index in 0..inventory.inventory_slots.len() {
        if inventory.inventory_slots[index].is_none() {
            inventory.inventory_slots[index] = Some(remaining);
            return None;
        }
    }

    Some(remaining)
}

fn slot_mut(
    inventory: &mut PlayerInventoryState,
    slot: ItemContainerSlot,
) -> Option<&mut Option<ItemStack>> {
    match slot.container {
        ItemContainer::Inventory => inventory.inventory_slots.get_mut(slot.slot),
        ItemContainer::Actionbar => inventory.actionbar_slots.get_mut(slot.slot),
    }
}

fn slot_exists(inventory: &PlayerInventoryState, slot: ItemContainerSlot) -> bool {
    (match slot.container {
        ItemContainer::Inventory => slot.slot < INVENTORY_SLOT_COUNT,
        ItemContainer::Actionbar => slot.slot < ACTIONBAR_SLOT_COUNT,
    }) && (match slot.container {
        ItemContainer::Inventory => slot.slot < inventory.inventory_slots.len(),
        ItemContainer::Actionbar => slot.slot < inventory.actionbar_slots.len(),
    })
}

fn offset_actionbar_slot(current: usize, offset: i8) -> usize {
    (current as isize + offset as isize).rem_euclid(ACTIONBAR_SLOT_COUNT as isize) as usize
}

fn drop_position(controller: &PlayerController) -> Vec3Net {
    let forward = Vec3Net::new(-controller.yaw.sin(), 0.0, -controller.yaw.cos());
    controller
        .position
        .plus(forward.scale(DROP_FORWARD_DISTANCE))
        .plus(Vec3Net::new(0.0, DROPPED_ITEM_GROUND_Y, 0.0))
}

fn player_eye_position(position: Vec3Net) -> Vec3Net {
    position.plus(Vec3Net::new(0.0, SERVER_EYE_HEIGHT, 0.0))
}

fn apply_client_movement(controller: &mut PlayerController, movement: PlayerMovement) {
    if movement.sequence <= controller.last_processed_input || !movement_is_finite(movement) {
        return;
    }

    controller.position = movement.position;
    controller.velocity = movement.velocity;
    controller.yaw = normalize_yaw(movement.yaw);
    controller.pitch = movement.pitch.clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
    controller.grounded = movement.grounded;
    controller.last_processed_input = movement.sequence;
}

fn movement_is_finite(movement: PlayerMovement) -> bool {
    vec3_is_finite(movement.position)
        && vec3_is_finite(movement.velocity)
        && movement.yaw.is_finite()
        && movement.pitch.is_finite()
}

fn vec3_is_finite(value: Vec3Net) -> bool {
    value.x.is_finite() && value.y.is_finite() && value.z.is_finite()
}

fn normalize_yaw(yaw: f32) -> f32 {
    (yaw + PI).rem_euclid(TAU) - PI
}

fn clean_player_name(name: &str, fallback_id: ClientId) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        format!("Player {fallback_id}")
    } else {
        trimmed.chars().take(32).collect()
    }
}

#[cfg(test)]
mod tests;
