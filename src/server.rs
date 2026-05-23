use std::collections::HashMap;

use crate::{
    controller::{BlockGrid, PlayerController},
    protocol::{
        CHAT_BUBBLE_DURATION_SECONDS, ChatMessage, ClientId, ClientMessage, DroppedItemId,
        PlayerInventoryState, ResourceNodeId, ResourceNodeState, SERVER_TICK_RATE_HZ,
        ServerMessage, SteamId, Vec3Net, sanitize_chat,
    },
    save::{PersistedPlayer, WorldSave},
    steam::AuthMode,
    world::WorldData,
    world_time::WorldTime,
};

const CLIENT_STALE_TIMEOUT_TICKS: u64 = 20 * 10;

/// How many ticks a chat bubble floats above the speaker before the server
/// clears it from snapshots. Derived from
/// [`CHAT_BUBBLE_DURATION_SECONDS`] so the visible lifetime is the same
/// regardless of tick rate.
const CHAT_BUBBLE_DURATION_TICKS: u64 = (CHAT_BUBBLE_DURATION_SECONDS * SERVER_TICK_RATE_HZ) as u64;

/// Cadence of the routine [`ServerMessage::WorldTime`] broadcast. One per
/// real minute keeps clients aligned against drift without flooding the
/// wire — the client integrates between snapshots using the same
/// multiplier, so the visible cycle stays smooth in between.
const WORLD_TIME_BROADCAST_INTERVAL_TICKS: u64 = (SERVER_TICK_RATE_HZ as u64) * 60;

mod commands;
mod connection;
mod dropped_items;
mod inventory;
mod movement;
mod persistence;
mod resource_nodes;
mod toasts;
mod voice;
mod world_time;

pub use voice::VOICE_AUDIBLE_RANGE;

use self::{
    dropped_items::{DROPPED_ITEM_MERGE_INTERVAL_TICKS, DroppedItemBody, DroppedItemPhysics},
    movement::accept_client_movement,
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
    /// Tear down the underlying transport session for this client. Emitted
    /// after a server-initiated `disconnect()` so the host layer can insert
    /// Lightyear's `Disconnecting` component and clear its connection map.
    /// Without this, kicked or stale clients hold their entity until the
    /// netcode timeout, and reconnects would be rejected as "already
    /// connected". The `message` field on the carrying envelope is ignored.
    Disconnect(ClientId),
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
    next_resource_node_id: ResourceNodeId,
    tick: u64,
    /// Authoritative day/night clock. Mirrored to clients via
    /// [`ServerMessage::WorldTime`]. Persisted to the save in `world_save`.
    world_time: WorldTime,
    /// Last tick a routine `WorldTime` broadcast was sent. Lets admin
    /// commands push an out-of-band immediate snapshot and reset this
    /// counter so the next routine broadcast is a full interval later.
    last_world_time_broadcast_tick: u64,
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
        // Floor at the static-spawn ceiling so a save authored before this
        // field existed (or one that's been hand-edited) can never hand out
        // an ID that collides with the world's hand-authored nodes.
        let next_resource_node_id = save
            .state
            .next_resource_node_id
            .max(resource_nodes.keys().copied().max().unwrap_or(0) + 1);
        let world_time = save.state.world_time();
        let tick = save.state.last_authoritative_tick;

        Self {
            tick,
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
            next_resource_node_id,
            world_time,
            last_world_time_broadcast_tick: tick,
        }
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
            ClientMessage::Chat { text } => {
                let Some(text) = sanitize_chat(&text) else {
                    return Vec::new();
                };
                let expires_tick = self.tick.saturating_add(CHAT_BUBBLE_DURATION_TICKS);
                let Some(client) = self.clients.get_mut(&client_id) else {
                    return Vec::new();
                };
                client.chat_bubble = Some(ChatBubble {
                    text: text.clone(),
                    expires_tick,
                });
                let from = client.name.clone();
                vec![ServerEnvelope {
                    target: DeliveryTarget::Broadcast,
                    message: ServerMessage::Chat(ChatMessage { from, text }),
                }]
            }
            ClientMessage::Command { text } => self.apply_command(client_id, text),
            ClientMessage::Inventory(command) => self.apply_inventory_command(client_id, command),
            ClientMessage::Gather(command) => self.apply_gather_command(client_id, command),
            ClientMessage::Voice(voice) => self.apply_voice_frame(client_id, voice),
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

    pub fn tick(&mut self, delta_seconds: f32) -> Vec<ServerEnvelope> {
        self.tick += 1;
        self.save.state.last_authoritative_tick = self.tick;
        self.world_time.advance(delta_seconds);
        self.dropped_item_physics
            .step(delta_seconds, &mut self.dropped_items);
        self.tick_resource_node_respawn(delta_seconds);
        self.expire_chat_bubbles();

        let mut envelopes = self.disconnect_stale_clients();
        if self.tick.is_multiple_of(DROPPED_ITEM_MERGE_INTERVAL_TICKS) {
            envelopes.extend(self.merge_nearby_dropped_items().into_iter().map(
                |(item_id, quantity)| ServerEnvelope {
                    target: DeliveryTarget::Broadcast,
                    message: ServerMessage::ItemMerged { item_id, quantity },
                },
            ));
        }

        if self
            .tick
            .saturating_sub(self.last_world_time_broadcast_tick)
            >= WORLD_TIME_BROADCAST_INTERVAL_TICKS
        {
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::WorldTime(self.world_time_snapshot()),
            });
            self.last_world_time_broadcast_tick = self.tick;
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

    fn expire_chat_bubbles(&mut self) {
        let tick = self.tick;
        for client in self.clients.values_mut() {
            if let Some(bubble) = &client.chat_bubble
                && bubble.expires_tick <= tick
            {
                client.chat_bubble = None;
            }
        }
    }
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
    /// Most recent chat line + the tick it stops being broadcast. Empty
    /// outside the bubble window. Snapshots copy `text` so peer clients can
    /// render speech bubbles above the speaker's head.
    pub(super) chat_bubble: Option<ChatBubble>,
}

#[derive(Debug, Clone)]
pub(super) struct ChatBubble {
    pub(super) text: String,
    pub(super) expires_tick: u64,
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
