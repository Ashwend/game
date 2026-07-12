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
        ChatMessage, ClientId, GAME_VERSION, PROTOCOL_VERSION, PlayerEvent, PlayerState,
        ServerMessage, Vec3Net,
    },
    save::WorldStore,
    world::{ChunkDims, WorldBlock, WorldData},
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
    /// World seed + chunk dims from `Welcome`'s `MapType`. The client renders
    /// the world-map terrain image locally from these (it's a pure function of
    /// the seed), so the server never ships the raster. `None` until `Welcome`.
    pub(crate) world_map_seed_dims: Option<(u64, ChunkDims)>,
    /// Spatial index over `world.blocks`. Rebuilt whenever a new world is
    /// installed (i.e. on `Welcome`). Lets prediction's substep loop query
    /// nearby blocks without scanning the full list.
    pub(crate) world_grid: Option<BlockGrid>,
    /// Monotonically increases every time `world` is replaced. The scene
    /// system uses this to detect "do I need to respawn world geometry?" in
    /// O(1) instead of deep-comparing the previous `WorldData`.
    pub(crate) world_version: u64,
    /// Footprints of placed deployables + buildings (NOT resource nodes), so the
    /// cosmetic detail grass can carve itself out of them, no grass poking through a
    /// foundation floor, furnace, or sleeping bag. Populated by
    /// `maintain_world_grid_system` (the deployable colliders it already computes).
    pub(crate) grass_displacers: Vec<WorldBlock>,
    /// Bumps whenever `grass_displacers` changes, so the grass streamer re-filters
    /// its field without polling. See [`Self::set_grass_displacers`].
    pub(crate) grass_displacer_version: u64,
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
    /// Estimated authoritative server tick. Seeded from each `WorldTime`
    /// broadcast's `server_tick` and advanced locally per frame, so the
    /// client can predict tick-based gates (e.g. the building demolish
    /// window) without a dedicated per-tick sync.
    pub(crate) server_tick_estimate: f64,
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
    /// Local player's most recent measured round-trip latency, in ms. Reported
    /// to the server on each `Ping` and shown in the pause-screen roster.
    pub(crate) local_ping_ms: u16,
    /// Latest connected-player roster from `ServerMessage::PlayerList`, name +
    /// ping for every online player (AoI-independent). Cleared on disconnect.
    pub(crate) players: Vec<crate::protocol::PlayerListEntry>,
    /// The live meteor shower event, if one has been announced. Seeded by a
    /// single `ServerMessage::MeteorShower` (resent to late joiners) and cleared
    /// once the client-side clock passes the crater despawn window. The sky
    /// visual, countdown HUD, danger warning, and temporary map marker all read
    /// this; nothing about the meteor is per-tick replicated.
    pub(crate) meteor_shower: Option<MeteorShowerEvent>,
}

/// Client-side mirror of an announced meteor shower event. A pure record of the
/// announce payload; all timing is derived on read against the authoritative
/// clock estimate ([`ClientRuntime::server_tick`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MeteorShowerEvent {
    /// Ground-zero world position (y is floor level).
    pub(crate) impact_position: crate::protocol::Vec3Net,
    /// Server tick the meteor strikes.
    pub(crate) impact_tick: u64,
    /// Seeds the fireball's approach azimuth (see `crate::world::meteor_shower`).
    pub(crate) trajectory_seed: u64,
    /// The client tick estimate at which this event stops being rendered (the
    /// crater and its map marker are removed). Derived from `impact_tick` plus
    /// the despawn window at announce time so the client cleans up without a
    /// second message.
    pub(crate) despawn_tick: u64,
}

impl MeteorShowerEvent {
    /// Real seconds until impact from the given clock estimate. Negative once the
    /// meteor has struck (the crater phase).
    pub(crate) fn seconds_to_impact(&self, estimated_tick: u64) -> f32 {
        (self.impact_tick as f64 - estimated_tick as f64) as f32
            / crate::protocol::SERVER_TICK_RATE_HZ
    }

    /// Whether the impact has already happened at the given clock estimate (the
    /// crater / shard phase; the fireball is gone).
    pub(crate) fn has_impacted(&self, estimated_tick: u64) -> bool {
        estimated_tick >= self.impact_tick
    }

    /// Whether the event is still live (pre-impact fireball or post-impact
    /// crater) at the given clock estimate. `false` once the crater despawns, at
    /// which point the runtime drops it.
    pub(crate) fn is_alive(&self, estimated_tick: u64) -> bool {
        estimated_tick < self.despawn_tick
    }
}

/// Surfaces a client-side error string as a toast. Emitted by any system
/// that has access to a `MessageWriter<ClientErrorToast>` (chat send, input
/// dispatch, network tick) so a single system,
/// `surface_client_error_toasts_system`, can be the only place that writes
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

    /// Whether every spawned save/shutdown has finished writing. Empty (no
    /// session ever shut down) counts as finished. Used by the update-apply
    /// path to hold the relaunch until an in-progress world save is durable.
    pub(crate) fn all_finished(&self) -> bool {
        self.0.iter().all(|task| task.is_finished())
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
        self.meteor_shower = None;
        self.connection.reset();
        self.world_time = WorldTime::default();
        self.server_tick_estimate = 0.0;
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
        self.world_map_seed_dims = None;
        self.world_grid = None;
        self.world_version = self.world_version.wrapping_add(1);
        self.predicted_local = None;
        self.is_admin = false;
        self.depleted_node_ids.clear();
        self.players.clear();
        self.meteor_shower = None;
        self.local_ping_ms = 0;
        self.connection.reset();
    }

    /// Record the latest measured round-trip latency. Called from the network
    /// tick (which has `Time`) when a `Pong` arrives.
    pub(crate) fn set_local_ping(&mut self, rtt_ms: u16) {
        self.local_ping_ms = rtt_ms;
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
        let mut grid = BlockGrid::build_with_extras(world, &extras);
        // Carry the live crater's analytic floor across rebuilds (the per-frame
        // sync in `tick_world_time` also covers the impact moment itself).
        grid.set_crater(self.impacted_crater_center());
        self.world_grid = Some(grid);
    }

    /// Ground-zero `(x, z)` of the live, already-impacted meteor shower crater, if
    /// any. Drives the movement grid's analytic floor so players walk over the
    /// crater mound; `None` before impact and once the event cleans up.
    fn impacted_crater_center(&self) -> Option<[f32; 2]> {
        self.meteor_shower
            .filter(|event| event.has_impacted(self.server_tick()))
            .map(|event| [event.impact_position.x, event.impact_position.z])
    }

    /// Replace the grass-displacer footprints (placed deployables/buildings only, not
    /// resource nodes) and bump the version so the detail-grass streamer re-filters its
    /// field. Called from `maintain_world_grid_system`, which is fingerprint-gated, so
    /// this only fires when the deployable set actually changes (never per-frame).
    pub(crate) fn set_grass_displacers(&mut self, blocks: Vec<WorldBlock>) {
        self.grass_displacers = blocks;
        self.grass_displacer_version = self.grass_displacer_version.wrapping_add(1);
    }

    pub(crate) fn apply_message(&mut self, message: ServerMessage) {
        // Any server-originated payload, including the periodic Heartbeat,
        // counts as proof the link is alive.
        self.connection.note_received();
        match message {
            ServerMessage::Welcome {
                client_id,
                map,
                world,
                is_admin,
                local_seed,
                world_time,
                ..
            } => {
                self.client_id = Some(client_id);
                self.is_admin = is_admin;
                // Stash the seed + dims so the client can render the world-map
                // terrain locally (it's a pure function of these).
                self.world_map_seed_dims = Some((map.world_seed(), map.chunk_dims()));
                self.world = Some(world);
                self.world_version = self.world_version.wrapping_add(1);
                self.seed_local_prediction(&local_seed);
                // The world collision grid is rebuilt by
                // `maintain_world_grid_system` once per frame,
                // bumping `world_version` (and the implicit
                // change to the resource-node set) is the signal
                // that triggers it.
                self.apply_world_time_snapshot(world_time);
                self.push_system_message(format!("connected as player {client_id}"));
            }
            ServerMessage::AuthRejected { reason } => {
                self.push_error_message(format!("auth rejected: {reason}"));
            }
            ServerMessage::VersionMismatch {
                server_version,
                server_protocol,
            } => {
                self.push_error_message(format!(
                    "version mismatch: server {server_version} (protocol {server_protocol}), \
                     client {GAME_VERSION} (protocol {PROTOCOL_VERSION})"
                ));
            }
            ServerMessage::Kicked { reason } => {
                self.push_error_message(format!("disconnected: {reason}"));
                self.clear_session_state();
            }
            ServerMessage::PlayerEvent(event) => {
                self.push_system_message(format_player_event(event));
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
                // system before reaching runtime state, no log/history
                // side-effect here.
            }
            ServerMessage::PlayerImpact { .. } => {
                // Fanned out to feedback events by the network tick
                // system. Runtime keeps no log of hits, they show
                // as floating damage, chip burst, and HP
                // replication.
            }
            ServerMessage::ProjectileImpact { .. } => {
                // Fanned out to a `RemoteImpactEvent` by the network tick system
                // for the arrow thunk/stick cue; runtime keeps no state for it.
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
            ServerMessage::DoorCodePrompt { .. } => {
                // Handled in the network tick system, which owns
                // `MenuState` and opens the code-entry dialog.
            }
            ServerMessage::DoorCodeResult { .. } => {
                // Handled in the network tick system, which plays the
                // keypad accept/deny sound.
            }
            ServerMessage::WorldTime(snapshot) => {
                self.apply_world_time_snapshot(snapshot);
            }
            ServerMessage::MeteorShower {
                impact_position,
                impact_tick,
                trajectory_seed,
            } => {
                // Store the announce; the sky/HUD/map systems derive everything
                // from it against the local clock estimate. The crater persists
                // for the despawn window after impact, matching the server, so a
                // late joiner who gets the resend during the crater phase still
                // sees the crater. Idempotent: a resend of the same event just
                // overwrites with identical data.
                let despawn_tick = impact_tick.saturating_add(
                    (crate::game_balance::METEOR_SHOWER_DESPAWN_SECONDS
                        * crate::protocol::SERVER_TICK_RATE_HZ) as u64,
                );
                self.meteor_shower = Some(MeteorShowerEvent {
                    impact_position,
                    impact_tick,
                    trajectory_seed,
                    despawn_tick,
                });
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
                // events by the network tick system before this point,
                // the runtime keeps no per-frame voice history.
            }
            ServerMessage::Pong { .. } => {
                // RTT is computed in the network tick (where `Time` is
                // available) and written via `set_local_ping`; nothing to log.
            }
            ServerMessage::PlayerList(entries) => {
                self.players = entries;
            }
            ServerMessage::WorldMapMarkers { .. } => {
                // Routed to `WorldMapState` by the network tick system. No
                // runtime history.
            }
            ServerMessage::Explosion { .. } => {
                // Cosmetic detonation cue; the flash / thump / rumble / screen
                // shake are driven off this in the network tick system (see
                // `network.rs`), and the authoritative blast already landed via
                // the replicated mirrors. Nothing to keep in runtime history.
            }
            ServerMessage::Heartbeat => {}
        }
    }

    fn apply_world_time_snapshot(&mut self, snapshot: WorldTimeSnapshot) {
        self.server_tick_estimate = snapshot.server_tick as f64;
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
        self.server_tick_estimate +=
            f64::from(delta_seconds.max(0.0)) * f64::from(crate::protocol::SERVER_TICK_RATE_HZ);
        // Drop a finished meteor shower event once its crater window closes on the
        // local clock estimate, matching the server's cleanup. The visuals key
        // on `runtime.meteor_shower`, so clearing it here removes the crater/marker.
        if let Some(event) = self.meteor_shower
            && !event.is_alive(self.server_tick())
        {
            self.meteor_shower = None;
        }
        // Keep the movement grid's analytic crater floor in step with the event
        // (installed the frame the impact lands, cleared when the event ends).
        // The grid rebuild path is fingerprint-gated on collider changes, so it
        // alone would miss the impact moment.
        let crater = self.impacted_crater_center();
        if let Some(grid) = self.world_grid.as_mut()
            && grid.crater() != crater
        {
            grid.set_crater(crater);
        }
    }

    /// Best estimate of the current authoritative server tick, advanced
    /// locally between the periodic `WorldTime` syncs. Used by client-side
    /// predictions of tick-based gates (the building demolish window).
    pub(crate) fn server_tick(&self) -> u64 {
        self.server_tick_estimate.max(0.0) as u64
    }

    /// The same clock estimate with its sub-tick fraction intact. Anything
    /// animating continuously off the server clock (the meteor shower's
    /// descent) must use this: truncating to a whole 20 Hz tick quantises
    /// motion into 50 ms steps, which stutters at render frame rates.
    pub(crate) fn server_tick_precise(&self) -> f64 {
        self.server_tick_estimate.max(0.0)
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
    /// meaningfully from the current prediction, a small per-tick drift
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

        // Position snap threshold, anything past this looks like an
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
mod tests;
