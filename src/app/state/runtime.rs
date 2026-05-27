use std::{
    collections::VecDeque,
    thread::{self, JoinHandle},
};

use bevy::prelude::*;
use uuid::Uuid;

use crate::{
    controller::{BlockGrid, PlayerController},
    net::ClientSession,
    protocol::{
        ChatMessage, ClientId, PlayerEvent, PlayerState, ServerMessage, Vec3Net, WorldSnapshot,
    },
    resources::resource_node_collider,
    save::WorldStore,
    world::{WorldBlock, WorldData},
    world_time::{WorldTime, WorldTimeSnapshot},
};

use super::connection::ConnectionWatch;

/// Knuth golden-ratio multiplier (`2^64 / phi`, odd). Mixes a XOR-of-ids
/// accumulator into a well-distributed `u64` fingerprint.
const COLLIDER_SET_HASH_MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// Cheap order-independent fingerprint of the live collider-bearing
/// entity set (trees + ores + placed deployables). Used by the snapshot
/// handler to skip rebuilding the collision grid when the set didn't
/// change. XOR of ids + count is good enough — the only way it collides
/// in practice is two entities being added and two different entities
/// being removed in the same tick, which can't happen during play.
pub(in crate::app) fn resource_node_collider_set_version(snapshot: Option<&WorldSnapshot>) -> u64 {
    let Some(snapshot) = snapshot else {
        return 0;
    };
    let mut hash: u64 = 0;
    let mut count: u64 = 0;
    for node in &snapshot.resource_nodes {
        if resource_node_collider(node).is_none() {
            continue;
        }
        hash ^= node.id;
        count += 1;
    }
    for entity in &snapshot.deployed_entities {
        // Same-id-space collision is impossible because deployables
        // and resource nodes draw from different counters server-side,
        // but flipping a bit keeps the two halves of the fingerprint
        // from accidentally cancelling out if they ever did share ids.
        hash ^= entity.id ^ 0xD9E3_F1A7_5B6C_8024;
        count += 1;
    }
    hash.wrapping_mul(COLLIDER_SET_HASH_MIX).wrapping_add(count)
}

/// AABB collider for a placed structure. Returns `None` if the item id
/// no longer resolves (e.g. a server using a newer item table than this
/// client knows about — in which case skip the collider rather than
/// crash, the renderer will still draw the structure).
fn deployable_collider(state: &crate::protocol::DeployedEntityState) -> Option<WorldBlock> {
    let profile = crate::items::item_definition(&state.item_id)?.deployable?;
    let center = Vec3Net::new(
        state.position.x,
        state.position.y + profile.collider_half_height,
        state.position.z,
    );
    let half = Vec3Net::new(
        profile.collider_half_width,
        profile.collider_half_height,
        profile.collider_half_width,
    );
    Some(WorldBlock::new(center, half))
}

pub(super) const MAX_CLIENT_LOG_MESSAGES: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientLogKind {
    System,
    Error,
    Chat { from: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientLogEntry {
    pub(crate) kind: ClientLogKind,
    pub(crate) text: String,
}

impl ClientLogEntry {
    fn system(text: impl Into<String>) -> Self {
        Self {
            kind: ClientLogKind::System,
            text: text.into(),
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            kind: ClientLogKind::Error,
            text: text.into(),
        }
    }

    fn chat(from: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: ClientLogKind::Chat { from: from.into() },
            text: text.into(),
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct ClientRuntime {
    pub(crate) session: Option<ClientSession>,
    pub(crate) active_world_id: Option<Uuid>,
    pub(crate) client_id: Option<ClientId>,
    pub(crate) is_admin: bool,
    pub(crate) world: Option<WorldData>,
    /// Spatial index over `world.blocks`. Rebuilt whenever a new world is
    /// installed (i.e. on `Welcome`). Lets prediction's substep loop query
    /// nearby blocks without scanning the full list.
    pub(crate) world_grid: Option<BlockGrid>,
    /// Monotonically increases every time `world` is replaced. The scene
    /// system uses this to detect "do I need to respawn world geometry?" in
    /// O(1) instead of deep-comparing the previous `WorldData`.
    pub(crate) world_version: u64,
    pub(crate) snapshot: Option<WorldSnapshot>,
    pub(crate) predicted_local: Option<PlayerController>,
    pub(crate) messages: VecDeque<ClientLogEntry>,
    pub(crate) input_sequence: u64,
    /// Hash of the live collider-bearing resource node set (trees + ores)
    /// used to detect when the `world_grid` needs to be rebuilt. Only
    /// changes when a node spawns or is exhausted — most snapshots keep
    /// the same set and skip the rebuild.
    pub(crate) resource_node_collider_version: u64,
    /// Tracks how long it's been since the server last sent anything. Used
    /// by the HUD's connection indicator. Lives in its own type so the
    /// thresholds and ticking logic stay focused on that concern.
    pub(crate) connection: ConnectionWatch,
    /// Client-side mirror of the authoritative day/night clock. Driven by
    /// the periodic `WorldTime` server broadcast plus per-frame local
    /// integration so the sun/moon position stays smooth between snapshots.
    pub(crate) world_time: WorldTime,
    /// Latest `ServerMessage::PerfStats` payload. Rendered by the perf
    /// HUD overlay when the user enables it in settings. `None` until the
    /// first broadcast arrives.
    pub(crate) perf_stats: Option<crate::protocol::PerfStatsSnapshot>,
    /// IDs of resource nodes the server has told us *actually* depleted
    /// (gathered out, picked up, admin-removed). Consumed by
    /// `apply_resource_nodes_system` to decide whether a snapshot-diff
    /// despawn should fire a death animation (tree-fell, ore shatter,
    /// crude pickup burst) or just silently disappear because the node
    /// only left this client's AoI ring.
    pub(crate) depleted_node_ids: std::collections::HashSet<crate::protocol::ResourceNodeId>,
}

/// Surfaces a client-side error string as a toast. Emitted by any system
/// that has access to a `MessageWriter<ClientErrorToast>` (chat send, input
/// dispatch, network tick) so a single system —
/// `surface_client_error_toasts_system` — can be the only place that writes
/// to `ToastState`. The runtime still keeps a copy in its chat log via
/// `push_error_message` for in-game history; this event is just for the
/// transient on-screen surface.
#[derive(Message, Debug, Clone)]
pub(crate) struct ClientErrorToast {
    pub(crate) text: String,
}

impl ClientErrorToast {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// Small abstraction over [`MessageWriter<ClientErrorToast>`] so UI and
/// input helpers can be unit-tested without spinning up a Bevy world.
/// Production code uses the blanket impl on `MessageWriter`; tests can
/// pass an `&mut Vec<String>` instead.
pub(crate) trait ErrorToastSink {
    fn push_error(&mut self, text: String);
}

impl<'w> ErrorToastSink for MessageWriter<'w, ClientErrorToast> {
    fn push_error(&mut self, text: String) {
        self.write(ClientErrorToast::new(text));
    }
}

impl ErrorToastSink for Vec<String> {
    fn push_error(&mut self, text: String) {
        self.push(text);
    }
}

#[derive(Resource, Default)]
pub(crate) struct SessionShutdownTasks(Vec<JoinHandle<Result<(), String>>>);

impl SessionShutdownTasks {
    pub(crate) fn spawn(&mut self, mut session: ClientSession, store: WorldStore) {
        match thread::Builder::new()
            .name("game-session-shutdown".to_owned())
            .spawn(move || {
                session
                    .shutdown(&store)
                    .map_err(|error| format!("{error:#}"))
            }) {
            Ok(task) => self.0.push(task),
            Err(error) => eprintln!("could not spawn game session shutdown: {error:#}"),
        }
    }

    pub(crate) fn drain_finished(&mut self) -> Vec<Result<(), String>> {
        let mut results = Vec::new();
        let mut pending = Vec::new();

        for task in self.0.drain(..) {
            if task.is_finished() {
                results.push(
                    task.join().unwrap_or_else(|_| {
                        Err("game session shutdown thread panicked".to_owned())
                    }),
                );
            } else {
                pending.push(task);
            }
        }

        self.0 = pending;
        results
    }

    #[cfg(test)]
    pub(super) fn push_finished_for_test(&mut self, result: Result<(), String>) {
        self.0.push(thread::spawn(move || result));
    }

    #[cfg(test)]
    pub(super) fn pending_len(&self) -> usize {
        self.0.len()
    }
}

impl ClientRuntime {
    /// True while a session is connected to a *remote* server. The signal is
    /// `session is some + active_world_id is none` because singleplayer
    /// always carries the loopback world's UUID; multiplayer never does.
    /// Used by the voice subsystem to gate microphone capture so it only
    /// opens when there's actually someone on the other end (which also
    /// keeps Bluetooth headsets in their high-quality A2DP profile while
    /// the player is in menus / singleplayer).
    pub(crate) fn is_multiplayer_session(&self) -> bool {
        self.session.is_some() && self.active_world_id.is_none()
    }

    pub(crate) fn start_session(&mut self, session: ClientSession, world_id: Option<Uuid>) {
        self.session = Some(session);
        self.active_world_id = world_id;
        self.client_id = None;
        self.is_admin = false;
        self.world = None;
        self.world_grid = None;
        self.world_version = self.world_version.wrapping_add(1);
        self.snapshot = None;
        self.predicted_local = None;
        self.messages.clear();
        self.input_sequence = 0;
        self.resource_node_collider_version = 0;
        self.depleted_node_ids.clear();
        self.connection.reset();
        self.world_time = WorldTime::default();
    }

    pub(crate) fn shutdown_in_background(
        &mut self,
        store: WorldStore,
        tasks: &mut SessionShutdownTasks,
    ) {
        if let Some(session) = self.session.take() {
            tasks.spawn(session, store);
        }
        self.clear_session_state();
    }

    fn clear_session_state(&mut self) {
        self.session = None;
        self.active_world_id = None;
        self.client_id = None;
        self.snapshot = None;
        self.world = None;
        self.world_grid = None;
        self.world_version = self.world_version.wrapping_add(1);
        self.predicted_local = None;
        self.is_admin = false;
        self.resource_node_collider_version = 0;
        self.depleted_node_ids.clear();
        self.connection.reset();
    }

    /// Rebuilds the world collision grid from the current world plus any
    /// live resource node colliders (tree trunks, ore rocks) present in
    /// the latest snapshot. Called after Welcome and whenever the live
    /// set of collider-bearing nodes changes (a node spawns, is felled,
    /// or is mined out).
    pub(in crate::app) fn rebuild_world_grid(&mut self) {
        let Some(world) = self.world.as_ref() else {
            self.world_grid = None;
            return;
        };
        let extras: Vec<WorldBlock> = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                let nodes = snapshot
                    .resource_nodes
                    .iter()
                    .filter_map(resource_node_collider);
                // Placed structures also block player movement. We
                // build the AABB from each structure's item profile so
                // the half-extents match the server-side overlap test.
                let deployables = snapshot
                    .deployed_entities
                    .iter()
                    .filter_map(deployable_collider);
                nodes.chain(deployables).collect()
            })
            .unwrap_or_default();
        self.world_grid = Some(BlockGrid::build_with_extras(world, &extras));
    }

    pub(crate) fn apply_message(&mut self, message: ServerMessage) {
        // Any server-originated payload — including the periodic Heartbeat —
        // counts as proof the link is alive.
        self.connection.note_received();
        match message {
            ServerMessage::Welcome {
                client_id,
                world,
                is_admin,
                snapshot,
                world_time,
                ..
            } => {
                self.client_id = Some(client_id);
                self.is_admin = is_admin;
                self.world = Some(world);
                self.world_version = self.world_version.wrapping_add(1);
                self.seed_local_prediction_from_snapshot(&snapshot, true);
                self.snapshot = Some(snapshot);
                self.rebuild_world_grid();
                self.resource_node_collider_version =
                    resource_node_collider_set_version(self.snapshot.as_ref());
                self.apply_world_time_snapshot(world_time);
                self.push_system_message(format!("connected as player {client_id}"));
            }
            ServerMessage::AuthRejected { reason } => {
                self.push_error_message(format!("auth rejected: {reason}"));
            }
            ServerMessage::Kicked { reason } => {
                self.push_error_message(format!("disconnected: {reason}"));
                self.clear_session_state();
            }
            ServerMessage::PlayerEvent(event) => {
                self.push_system_message(format_player_event(event))
            }
            ServerMessage::Correction(player) => {
                self.apply_non_movement_correction(&player);
            }
            ServerMessage::Chat(ChatMessage { from, text }) => {
                self.push_chat_message(from, text);
            }
            ServerMessage::ItemMerged { .. } => {}
            ServerMessage::Toast(_) => {
                // Toasts are routed straight to `ToastState` by the network
                // tick system so they reach the UI without touching client
                // log history. `apply_message` is intentionally a no-op here.
            }
            ServerMessage::ResourceImpact { .. } => {
                // Fanned out to `RemoteImpactEvent` by the network tick
                // system before reaching runtime state — no log/history
                // side-effect here.
            }
            ServerMessage::WorldTime(snapshot) => {
                self.apply_world_time_snapshot(snapshot);
            }
            ServerMessage::PerfStats(stats) => {
                self.perf_stats = Some(stats);
            }
            ServerMessage::ResourceNodeDepleted { id } => {
                // Mark for "real" death animation. The consumer
                // (`apply_resource_nodes_system`) removes the ID when it
                // processes the despawn; anything left over (e.g., the
                // node already left AoI when this arrived) lingers
                // harmlessly until the player reconnects.
                self.depleted_node_ids.insert(id);
            }
            ServerMessage::Voice { .. } => {
                // Voice frames are dispatched as `IncomingVoiceMessage`
                // events by the network tick system before this point —
                // the runtime keeps no per-frame voice history.
            }
            ServerMessage::Heartbeat => {}
        }
    }

    fn apply_world_time_snapshot(&mut self, snapshot: WorldTimeSnapshot) {
        self.world_time.seconds_of_day = snapshot.seconds_of_day;
        self.world_time.multiplier = snapshot.multiplier;
        // Re-clamp on the read side: a future server might broadcast a
        // value outside our tolerated range and we don't want a stray
        // negative multiplier driving the local integrator.
        self.world_time.set_seconds(self.world_time.seconds_of_day);
        self.world_time.set_multiplier(self.world_time.multiplier);
    }

    /// Advance the client-side mirror of the day/night clock by one frame
    /// of real time. Called from the network tick so the sun/moon visuals
    /// keep moving smoothly between the server's ~minute snapshots.
    pub(crate) fn tick_world_time(&mut self, delta_seconds: f32) {
        self.world_time.advance(delta_seconds);
    }

    pub(crate) fn push_system_message(&mut self, text: impl Into<String>) {
        self.push_message(ClientLogEntry::system(text));
    }

    /// Append an error to the chat log only. Callers that also want a
    /// transient toast should write a [`ClientErrorToast`] event alongside
    /// this call; the dedicated toast-surfacing system will pick it up.
    /// Keeping the log push and the toast push as two explicit calls makes
    /// the visibility of each side-effect obvious at the call site.
    pub(crate) fn push_error_message(&mut self, text: impl Into<String>) {
        self.push_message(ClientLogEntry::error(text));
    }

    /// Returns true when the session has gone long enough without a server
    /// message that the connection should be flagged as suspect. Only
    /// meaningful while a session is active.
    pub(crate) fn connection_is_lagging(&self) -> bool {
        self.connection.is_lagging(self.session.is_some())
    }

    /// Step the "time since last server message" counter. Called from the
    /// network tick. Wall-clock seconds since the last successful receive.
    pub(crate) fn tick_connection_silence(&mut self, delta_seconds: f32) {
        self.connection.tick(delta_seconds, self.session.is_some());
    }

    pub(crate) fn push_chat_message(&mut self, from: impl Into<String>, text: impl Into<String>) {
        self.push_message(ClientLogEntry::chat(from, text));
    }

    pub(crate) fn stop_session_after_kick(&mut self) {
        self.session = None;
        self.clear_session_state();
    }

    fn push_message(&mut self, message: ClientLogEntry) {
        self.messages.push_back(message);
        while self.messages.len() > MAX_CLIENT_LOG_MESSAGES {
            self.messages.pop_front();
        }
    }

    pub(crate) fn local_player(&self) -> Option<&PlayerState> {
        let client_id = self.client_id?;
        self.snapshot
            .as_ref()?
            .players
            .iter()
            .find(|player| player.client_id == client_id)
    }

    /// Best-known world-space position of the local player's feet.
    /// Prefers the predicted controller (zero-latency placement preview)
    /// and falls back to the last server snapshot.
    pub(crate) fn local_player_position(&self) -> Option<Vec3> {
        if let Some(predicted) = &self.predicted_local {
            return Some(predicted.position.into());
        }
        let player = self.local_player()?;
        Some(player.position.into())
    }

    pub(crate) fn local_view(&self) -> Option<LocalPlayerView> {
        if let Some(predicted) = &self.predicted_local {
            return Some(LocalPlayerView {
                position: predicted.view_position(),
                yaw: predicted.yaw,
                pitch: predicted.pitch,
                health: predicted.health,
            });
        }

        let player = self.local_player()?;
        Some(LocalPlayerView {
            position: player.position,
            yaw: player.yaw,
            pitch: player.pitch,
            health: player.health,
        })
    }

    pub(super) fn seed_local_prediction_from_snapshot(
        &mut self,
        snapshot: &WorldSnapshot,
        force: bool,
    ) {
        let Some(client_id) = self.client_id else {
            return;
        };
        let Some(server_player) = snapshot
            .players
            .iter()
            .find(|player| player.client_id == client_id)
        else {
            return;
        };

        if force || self.predicted_local.is_none() {
            self.predicted_local = Some(PlayerController::from_player_state(server_player));
            self.input_sequence = self.input_sequence.max(server_player.last_processed_input);
        }
    }

    fn apply_non_movement_correction(&mut self, player: &PlayerState) {
        if Some(player.client_id) != self.client_id {
            return;
        }

        if let Some(predicted) = &mut self.predicted_local {
            predicted.health = player.health;
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalPlayerView {
    pub(crate) position: Vec3Net,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) health: f32,
}

fn format_player_event(event: PlayerEvent) -> String {
    match event {
        PlayerEvent::Joined { name, .. } => format!("{name} joined"),
        PlayerEvent::Left { name, .. } => format!("{name} left"),
    }
}
