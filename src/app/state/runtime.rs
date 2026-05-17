use std::thread::{self, JoinHandle};

use bevy::prelude::*;
use uuid::Uuid;

use crate::{
    controller::PlayerController,
    net::ClientSession,
    protocol::{
        ChatMessage, ClientId, PlayerEvent, PlayerState, ServerMessage, Vec3Net, WorldSnapshot,
    },
    save::WorldStore,
    world::WorldData,
};

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
    pub(crate) snapshot: Option<WorldSnapshot>,
    pub(crate) predicted_local: Option<PlayerController>,
    pub(crate) messages: Vec<ClientLogEntry>,
    pub(crate) input_sequence: u64,
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
        self.snapshot = None;
        self.predicted_local = None;
        self.messages.clear();
        self.input_sequence = 0;
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
        self.predicted_local = None;
        self.is_admin = false;
    }

    pub(crate) fn apply_message(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Welcome {
                client_id,
                world,
                is_admin,
                snapshot,
                ..
            } => {
                self.client_id = Some(client_id);
                self.is_admin = is_admin;
                self.world = Some(world);
                self.seed_local_prediction_from_snapshot(&snapshot, true);
                self.snapshot = Some(snapshot);
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
            }
            ServerMessage::Correction(player) => {
                self.apply_non_movement_correction(&player);
            }
            ServerMessage::Chat(ChatMessage { from, text }) => {
                self.push_chat_message(from, text);
            }
            ServerMessage::ItemMerged { .. } => {}
            ServerMessage::Heartbeat => {}
        }
    }

    pub(crate) fn push_system_message(&mut self, text: impl Into<String>) {
        self.push_message(ClientLogEntry::system(text));
    }

    pub(crate) fn push_error_message(&mut self, text: impl Into<String>) {
        self.push_message(ClientLogEntry::error(text));
    }

    pub(crate) fn push_chat_message(&mut self, from: impl Into<String>, text: impl Into<String>) {
        self.push_message(ClientLogEntry::chat(from, text));
    }

    pub(crate) fn stop_session_after_kick(&mut self) {
        self.session = None;
        self.clear_session_state();
    }

    fn push_message(&mut self, message: ClientLogEntry) {
        self.messages.push(message);

        if self.messages.len() > MAX_CLIENT_LOG_MESSAGES {
            let drain_count = self.messages.len() - MAX_CLIENT_LOG_MESSAGES;
            self.messages.drain(0..drain_count);
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
