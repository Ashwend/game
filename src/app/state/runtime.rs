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
/// resource node set (trees + ores). Used by the snapshot handler to skip
/// rebuilding the collision grid when the set didn't change. XOR of node
/// IDs + count is good enough — the only way it collides in practice is
/// two nodes being added and two different nodes being removed in the
/// same tick, which can't happen during play.
fn resource_node_collider_set_version(snapshot: Option<&WorldSnapshot>) -> u64 {
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
    hash.wrapping_mul(COLLIDER_SET_HASH_MIX).wrapping_add(count)
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
        self.connection.reset();
    }

    /// Rebuilds the world collision grid from the current world plus any
    /// live resource node colliders (tree trunks, ore rocks) present in
    /// the latest snapshot. Called after Welcome and whenever the live
    /// set of collider-bearing nodes changes (a node spawns, is felled,
    /// or is mined out).
    fn rebuild_world_grid(&mut self) {
        let Some(world) = self.world.as_ref() else {
            self.world_grid = None;
            return;
        };
        let extras: Vec<WorldBlock> = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .resource_nodes
                    .iter()
                    .filter_map(resource_node_collider)
                    .collect()
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
            ServerMessage::Snapshot(snapshot) => {
                if self.is_stale_snapshot(snapshot.tick) {
                    return;
                }
                self.snapshot = Some(snapshot);
                // Nodes can disappear between snapshots (trees felled,
                // ores mined out). Rebuild the collision grid only when
                // the live set actually changes — every snapshot would
                // be wasted work.
                let new_version = resource_node_collider_set_version(self.snapshot.as_ref());
                if new_version != self.resource_node_collider_version {
                    self.resource_node_collider_version = new_version;
                    self.rebuild_world_grid();
                }
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

    fn is_stale_snapshot(&self, tick: u64) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|current| tick <= current.tick)
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
