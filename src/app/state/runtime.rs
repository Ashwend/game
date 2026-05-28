use std::{
    collections::VecDeque,
    thread::{self, JoinHandle},
};

use bevy::prelude::*;
use uuid::Uuid;

use crate::{
    controller::{BlockGrid, PlayerController},
    net::ClientSession,
    protocol::{ChatMessage, ClientId, PlayerEvent, PlayerState, ServerMessage, Vec3Net},
    save::WorldStore,
    world::{WorldBlock, WorldData},
    world_time::{WorldTime, WorldTimeSnapshot},
};

use super::connection::ConnectionWatch;

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
    pub(crate) predicted_local: Option<PlayerController>,
    pub(crate) messages: VecDeque<ClientLogEntry>,
    pub(crate) input_sequence: u64,
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
        self.predicted_local = None;
        self.messages.clear();
        self.input_sequence = 0;
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
        self.world = None;
        self.world_grid = None;
        self.world_version = self.world_version.wrapping_add(1);
        self.predicted_local = None;
        self.is_admin = false;
        self.depleted_node_ids.clear();
        self.connection.reset();
    }

    /// Rebuilds the world collision grid from the current world plus
    /// live resource-node and deployable colliders supplied by the
    /// caller (a per-frame system that watches replicated entities).
    pub(crate) fn rebuild_world_grid<R, D>(
        &mut self,
        resource_node_colliders: R,
        deployable_colliders: D,
    ) where
        R: IntoIterator<Item = WorldBlock>,
        D: IntoIterator<Item = WorldBlock>,
    {
        let Some(world) = self.world.as_ref() else {
            self.world_grid = None;
            return;
        };
        // Trees + ores from the replicated resource-node set, plus
        // placed structures from the replicated deployable set. The
        // collider half-extents come from each entity's definition /
        // item profile to match the server-side overlap test.
        let mut extras: Vec<WorldBlock> = resource_node_colliders.into_iter().collect();
        extras.extend(deployable_colliders);
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
                local_seed,
                world_time,
                ..
            } => {
                self.client_id = Some(client_id);
                self.is_admin = is_admin;
                self.world = Some(world);
                self.world_version = self.world_version.wrapping_add(1);
                self.seed_local_prediction(&local_seed);
                // The world collision grid is rebuilt by
                // `maintain_world_grid_system` once per frame —
                // bumping `world_version` (and the implicit
                // change to the resource-node set) is the signal
                // that triggers it.
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
            ServerMessage::PlayerImpact { .. } => {
                // Fanned out to feedback events by the network tick
                // system. Runtime keeps no log of hits — they show
                // as floating damage, chip burst, and HP
                // replication.
            }
            ServerMessage::Knockback { impulse } => {
                // Apply the server-authored impulse directly to the
                // local prediction's velocity. A cheater ignoring
                // this message only forfeits their own pushback.
                if let Some(predicted) = &mut self.predicted_local {
                    predicted.velocity = predicted.velocity.plus(impulse);
                    // Knockback should always lift the target off the
                    // ground for one frame so the upward fraction in
                    // the impulse actually carries; without this, the
                    // controller's "grounded → cap y at 0" branch
                    // would eat the vertical component on the next
                    // substep.
                    predicted.grounded = false;
                }
            }
            ServerMessage::PlayerKilled { .. } => {
                // The death splash itself lives on `MenuState`; the
                // dedicated `route_player_killed_system` opens it.
                // Runtime only logs the event so the chat history
                // shows what happened.
                self.push_system_message("you died".to_owned());
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

    /// Best-known world-space position of the local player's feet.
    /// The predicted controller is the source of truth from Welcome
    /// onward; before Welcome there's no view at all.
    pub(crate) fn local_player_position(&self) -> Option<Vec3> {
        self.predicted_local
            .as_ref()
            .map(|predicted| predicted.position.into())
    }

    pub(crate) fn local_view(&self) -> Option<LocalPlayerView> {
        let predicted = self.predicted_local.as_ref()?;
        Some(LocalPlayerView {
            position: predicted.view_position(),
            yaw: predicted.yaw,
            pitch: predicted.pitch,
            health: predicted.health,
        })
    }

    /// Initialise `predicted_local` from the Welcome message's
    /// `local_seed` field. Welcome carries exactly the fields
    /// prediction needs (`PlayerState`); remote players arrive via
    /// Lightyear replication, not Welcome.
    pub(super) fn seed_local_prediction(&mut self, seed: &PlayerState) {
        self.predicted_local = Some(PlayerController::from_player_state(seed));
        self.input_sequence = self.input_sequence.max(seed.last_processed_input);
    }

    /// Server-authoritative correction of the local prediction. Health
    /// is always overwritten (the server is the source of truth for
    /// damage). Position/velocity/yaw/pitch only snap when they differ
    /// meaningfully from the current prediction — a small per-tick drift
    /// shouldn't yank the player off-screen. The teleport, respawn, and
    /// future anti-cheat snap-back paths use this to force a full state
    /// reset by sending a `PlayerState` that diverges from the predicted
    /// values.
    fn apply_non_movement_correction(&mut self, player: &PlayerState) {
        if Some(player.client_id) != self.client_id {
            return;
        }

        let Some(predicted) = &mut self.predicted_local else {
            return;
        };
        predicted.health = player.health;

        // Position snap threshold — anything past this looks like an
        // intentional server-side relocation (teleport, respawn) rather
        // than a small floating-point drift. 1 m is bigger than any
        // single-tick movement the controller can produce at run speed,
        // and small enough that a real desync still corrects.
        const SNAP_THRESHOLD_M: f32 = 1.0;
        let dx = predicted.position.x - player.position.x;
        let dy = predicted.position.y - player.position.y;
        let dz = predicted.position.z - player.position.z;
        if (dx * dx + dy * dy + dz * dz).sqrt() > SNAP_THRESHOLD_M {
            *predicted = PlayerController::from_player_state(player);
            self.input_sequence = self.input_sequence.max(player.last_processed_input);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MAX_HEALTH, PlayerState};

    fn player_state(client_id: ClientId, position: Vec3Net) -> PlayerState {
        PlayerState {
            client_id,
            position,
            velocity: Vec3Net::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            health: MAX_HEALTH,
            grounded: true,
            last_processed_input: 0,
        }
    }

    #[test]
    fn message_log_caps_at_the_max_and_drops_oldest_first() {
        let mut runtime = ClientRuntime::default();
        for i in 0..(MAX_CLIENT_LOG_MESSAGES + 10) {
            runtime.push_system_message(format!("msg {i}"));
        }
        assert_eq!(runtime.messages.len(), MAX_CLIENT_LOG_MESSAGES);
        // Oldest entries fall off the front, newest remain.
        assert_eq!(runtime.messages.front().unwrap().text, "msg 10");
        assert_eq!(
            runtime.messages.back().unwrap().text,
            format!("msg {}", MAX_CLIENT_LOG_MESSAGES + 9)
        );
    }

    #[test]
    fn push_helpers_tag_the_log_kind() {
        let mut runtime = ClientRuntime::default();
        runtime.push_system_message("sys");
        runtime.push_error_message("err");
        runtime.push_chat_message("Alice", "hi");

        assert_eq!(runtime.messages[0].kind, ClientLogKind::System);
        assert_eq!(runtime.messages[1].kind, ClientLogKind::Error);
        assert_eq!(
            runtime.messages[2].kind,
            ClientLogKind::Chat {
                from: "Alice".to_owned()
            }
        );
        assert_eq!(runtime.messages[2].text, "hi");
    }

    #[test]
    fn welcome_seeds_prediction_admin_flag_and_world() {
        let mut runtime = ClientRuntime::default();
        let before_version = runtime.world_version;
        runtime.apply_message(ServerMessage::Welcome {
            client_id: 42,
            map: crate::world::MapType::default(),
            world: WorldData::default(),
            is_admin: true,
            local_seed: player_state(42, Vec3Net::new(5.0, 0.0, -3.0)),
            world_time: WorldTimeSnapshot {
                seconds_of_day: 100.0,
                multiplier: 1.0,
                server_tick: 0,
            },
        });

        assert_eq!(runtime.client_id, Some(42));
        assert!(runtime.is_admin);
        assert!(runtime.world.is_some());
        assert!(runtime.predicted_local.is_some());
        assert_eq!(runtime.world_version, before_version + 1);
        // Connection log entry written.
        assert!(
            runtime
                .messages
                .iter()
                .any(|m| m.text.contains("connected as player 42"))
        );
    }

    #[test]
    fn kicked_logs_error_and_clears_session_state() {
        let mut runtime = ClientRuntime::default();
        runtime.apply_message(ServerMessage::Welcome {
            client_id: 1,
            map: crate::world::MapType::default(),
            world: WorldData::default(),
            is_admin: true,
            local_seed: player_state(1, Vec3Net::ZERO),
            world_time: WorldTimeSnapshot {
                seconds_of_day: 0.0,
                multiplier: 1.0,
                server_tick: 0,
            },
        });
        runtime.apply_message(ServerMessage::Kicked {
            reason: "afk".to_owned(),
        });

        assert!(runtime.client_id.is_none(), "kick clears the client id");
        assert!(runtime.world.is_none(), "kick clears the world");
        assert!(runtime.predicted_local.is_none());
        assert!(!runtime.is_admin);
        assert!(
            runtime
                .messages
                .iter()
                .any(|m| m.kind == ClientLogKind::Error && m.text.contains("afk"))
        );
    }

    #[test]
    fn knockback_adds_impulse_and_lifts_off_ground() {
        let mut runtime = ClientRuntime::default();
        runtime.seed_local_prediction(&player_state(1, Vec3Net::ZERO));
        runtime.client_id = Some(1);
        runtime.predicted_local.as_mut().unwrap().grounded = true;
        runtime.predicted_local.as_mut().unwrap().velocity = Vec3Net::ZERO;

        runtime.apply_message(ServerMessage::Knockback {
            impulse: Vec3Net::new(2.0, 3.0, -1.0),
        });

        let predicted = runtime.predicted_local.as_ref().unwrap();
        assert_eq!(predicted.velocity.x, 2.0);
        assert_eq!(predicted.velocity.y, 3.0);
        assert_eq!(predicted.velocity.z, -1.0);
        assert!(
            !predicted.grounded,
            "knockback must lift the player so the upward impulse carries"
        );
    }

    #[test]
    fn world_time_message_clamps_and_updates_the_mirror() {
        let mut runtime = ClientRuntime::default();
        runtime.apply_message(ServerMessage::WorldTime(WorldTimeSnapshot {
            seconds_of_day: 3600.0,
            // A negative multiplier must be re-clamped on read.
            multiplier: -5.0,
            server_tick: 0,
        }));
        assert_eq!(runtime.world_time.seconds_of_day, 3600.0);
        assert!(
            runtime.world_time.multiplier >= 0.0,
            "negative multiplier must be clamped to the tolerated range"
        );
    }

    #[test]
    fn resource_node_depleted_marks_id_for_death_animation() {
        let mut runtime = ClientRuntime::default();
        runtime.apply_message(ServerMessage::ResourceNodeDepleted { id: 9 });
        assert!(runtime.depleted_node_ids.contains(&9));
    }

    #[test]
    fn player_killed_logs_you_died() {
        let mut runtime = ClientRuntime::default();
        runtime.apply_message(ServerMessage::PlayerKilled {
            killer: Some(2),
            killer_name: Some("Bob".to_owned()),
        });
        assert!(runtime.messages.iter().any(|m| m.text == "you died"));
    }

    #[test]
    fn correction_snaps_only_past_the_threshold() {
        let mut runtime = ClientRuntime::default();
        runtime.seed_local_prediction(&player_state(1, Vec3Net::new(0.0, 0.0, 0.0)));
        runtime.client_id = Some(1);

        // Sub-threshold position delta (0.5 m) → no snap, but health is
        // always overwritten.
        let mut small = player_state(1, Vec3Net::new(0.5, 0.0, 0.0));
        small.health = 30.0;
        runtime.apply_message(ServerMessage::Correction(small));
        let predicted = runtime.predicted_local.as_ref().unwrap();
        assert!(
            predicted.position.x.abs() < 0.01,
            "small drift must not snap the predicted position"
        );
        assert_eq!(predicted.health, 30.0, "health always follows the server");

        // Large delta (10 m) → full snap to the corrected state.
        let big = player_state(1, Vec3Net::new(10.0, 0.0, 0.0));
        runtime.apply_message(ServerMessage::Correction(big));
        assert!(
            (runtime.predicted_local.as_ref().unwrap().position.x - 10.0).abs() < 0.01,
            "a large divergence must snap the prediction to the server state"
        );
    }

    #[test]
    fn correction_for_a_different_client_is_ignored() {
        let mut runtime = ClientRuntime::default();
        runtime.seed_local_prediction(&player_state(1, Vec3Net::ZERO));
        runtime.client_id = Some(1);
        let mut other = player_state(2, Vec3Net::new(99.0, 0.0, 0.0));
        other.health = 1.0;

        runtime.apply_message(ServerMessage::Correction(other));

        let predicted = runtime.predicted_local.as_ref().unwrap();
        assert!(predicted.position.x.abs() < 0.01);
        assert_eq!(
            predicted.health, MAX_HEALTH,
            "a correction targeting another client must not touch our state"
        );
    }

    #[test]
    fn is_multiplayer_session_requires_session_without_world_id() {
        let mut runtime = ClientRuntime::default();
        // No session at all → not multiplayer.
        assert!(!runtime.is_multiplayer_session());
        // Simulate a remote session: session present + no world id. We
        // can't fabricate a real ClientSession, so assert the world-id
        // branch directly via the helper's logic preconditions.
        runtime.active_world_id = Some(Uuid::nil());
        assert!(
            !runtime.is_multiplayer_session(),
            "a world id (singleplayer) is never a multiplayer session"
        );
    }

    #[test]
    fn local_view_and_position_track_prediction() {
        let mut runtime = ClientRuntime::default();
        assert!(runtime.local_view().is_none());
        assert!(runtime.local_player_position().is_none());

        runtime.seed_local_prediction(&player_state(1, Vec3Net::new(1.0, 2.0, 3.0)));
        let view = runtime.local_view().expect("view present after seed");
        assert_eq!(view.health, MAX_HEALTH);
        let pos = runtime.local_player_position().expect("position present");
        assert_eq!(pos.x, 1.0);
        assert_eq!(pos.z, 3.0);
    }

    #[test]
    fn error_toast_sink_vec_impl_collects_text() {
        let mut sink: Vec<String> = Vec::new();
        sink.push_error("boom".to_owned());
        sink.push_error("again".to_owned());
        assert_eq!(sink, vec!["boom".to_owned(), "again".to_owned()]);
    }

    #[test]
    fn shutdown_tasks_drain_only_returns_finished() {
        let mut tasks = SessionShutdownTasks::default();
        tasks.push_finished_for_test(Ok(()));
        tasks.push_finished_for_test(Err("nope".to_owned()));
        // Allow the threads to finish.
        let mut drained = Vec::new();
        for _ in 0..100 {
            drained = tasks.drain_finished();
            if !drained.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Once finished, both results surface and the queue empties.
        let all: Vec<_> = {
            let mut acc = drained;
            acc.extend(tasks.drain_finished());
            acc
        };
        assert!(all.iter().any(|r| r.is_ok()));
        assert!(all.iter().any(|r| r.is_err()));
        assert_eq!(tasks.pending_len(), 0);
    }
}
