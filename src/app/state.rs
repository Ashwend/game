use bevy::prelude::*;
use uuid::Uuid;

use crate::{
    controller::PlayerController,
    net::ClientSession,
    protocol::{
        ChatMessage, ClientId, PlayerEvent, PlayerState, ServerMessage, Vec3Net, WorldSnapshot,
    },
    save::{WorldStore, WorldSummary},
    steam::AuthenticatedUser,
    world::WorldData,
};

const MAX_CLIENT_LOG_MESSAGES: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    MainMenu,
    Worlds,
    #[expect(
        dead_code,
        reason = "The multiplayer screen is built but gated behind a coming-soon menu entry."
    )]
    Multiplayer,
    InGame,
}

#[derive(Resource)]
pub(crate) struct SaveStore(pub(crate) WorldStore);

#[derive(Resource)]
pub(crate) struct SteamUser(pub(crate) AuthenticatedUser);

#[derive(Resource)]
pub(crate) struct MenuState {
    pub(crate) screen: Screen,
    pub(crate) worlds: Vec<WorldSummary>,
    pub(crate) new_world_name: String,
    pub(crate) multiplayer_addr: String,
    pub(crate) status: Option<String>,
    pub(crate) pause_open: bool,
    pub(crate) chat_open: bool,
    pub(crate) chat_focus_pending: bool,
    pub(crate) chat_input: String,
    pub(crate) confirmation: Option<ConfirmationDialog>,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            screen: Screen::MainMenu,
            worlds: Vec::new(),
            new_world_name: "New World".to_owned(),
            multiplayer_addr: "127.0.0.1:7777".to_owned(),
            status: None,
            pause_open: false,
            chat_open: false,
            chat_focus_pending: false,
            chat_input: String::new(),
            confirmation: None,
        }
    }
}

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

#[derive(Debug, Clone)]
pub(crate) struct ConfirmationDialog {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) confirm_label: String,
    pub(crate) cancel_label: String,
    pub(crate) action: ConfirmationAction,
    pub(crate) closing: bool,
    pub(crate) confirmed: bool,
}

impl ConfirmationDialog {
    pub(crate) fn delete_world(world_id: Uuid, world_name: &str) -> Self {
        Self {
            title: "Delete World".to_owned(),
            body: format!("Permanently delete \"{world_name}\"? This cannot be undone."),
            confirm_label: "Delete".to_owned(),
            cancel_label: "Cancel".to_owned(),
            action: ConfirmationAction::DeleteWorld { world_id },
            closing: false,
            confirmed: false,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ConfirmationAction {
    DeleteWorld { world_id: Uuid },
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

    pub(crate) fn shutdown(&mut self, store: &WorldStore) {
        if let Some(session) = self.session.as_mut()
            && let Err(error) = session.shutdown(store)
        {
            self.push_error_message(format!("save/shutdown error: {error}"));
        }

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
            ServerMessage::PlayerEvent(event) => {
                self.push_system_message(format_player_event(event))
            }
            ServerMessage::Snapshot(snapshot) => {
                if self.is_stale_snapshot(snapshot.tick) {
                    return;
                }
                self.seed_local_prediction_from_snapshot(&snapshot, false);
                self.snapshot = Some(snapshot);
            }
            ServerMessage::Correction(player) => {
                self.apply_non_movement_correction(&player);
            }
            ServerMessage::Chat(ChatMessage { from, text }) => {
                self.push_chat_message(from, text);
            }
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
                position: predicted.position,
                health: predicted.health,
            });
        }

        let player = self.local_player()?;
        Some(LocalPlayerView {
            position: player.position,
            health: player.health,
        })
    }

    fn seed_local_prediction_from_snapshot(&mut self, snapshot: &WorldSnapshot, force: bool) {
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
    pub(crate) health: f32,
}

#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct LookState {
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) sensitivity: Vec2,
}

impl Default for LookState {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: -0.04,
            sensitivity: Vec2::new(0.0024, 0.0020),
        }
    }
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
    use crate::protocol::MAX_HEALTH;

    fn player_state(client_id: ClientId, position: Vec3Net) -> PlayerState {
        PlayerState {
            client_id,
            steam_id: client_id,
            name: format!("Player {client_id}"),
            position,
            velocity: Vec3Net::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            health: MAX_HEALTH,
            grounded: true,
            last_processed_input: 0,
            is_admin: false,
        }
    }

    #[test]
    fn welcome_seeds_local_prediction_from_snapshot() {
        let mut server_player = player_state(1, Vec3Net::new(2.0, 0.0, 0.0));
        server_player.last_processed_input = 7;
        let mut runtime = ClientRuntime {
            client_id: Some(1),
            ..default()
        };

        runtime.seed_local_prediction_from_snapshot(
            &WorldSnapshot {
                tick: 1,
                players: vec![server_player],
            },
            true,
        );

        let predicted = runtime.predicted_local.expect("prediction should exist");
        assert_eq!(predicted.position, Vec3Net::new(2.0, 0.0, 0.0));
        assert_eq!(runtime.input_sequence, 7);
    }

    #[test]
    fn snapshots_do_not_reconcile_existing_local_prediction() {
        let mut runtime = ClientRuntime {
            client_id: Some(1),
            predicted_local: Some(PlayerController::from_player_state(&player_state(
                1,
                Vec3Net::new(5.0, 0.0, 0.0),
            ))),
            ..default()
        };

        runtime.apply_message(ServerMessage::Snapshot(WorldSnapshot {
            tick: 1,
            players: vec![player_state(1, Vec3Net::ZERO)],
        }));

        let predicted = runtime.predicted_local.expect("prediction should exist");
        assert_eq!(predicted.position, Vec3Net::new(5.0, 0.0, 0.0));
        assert_eq!(runtime.snapshot.expect("snapshot should exist").tick, 1);
    }

    #[test]
    fn stale_snapshots_are_ignored() {
        let current_snapshot = WorldSnapshot {
            tick: 5,
            players: vec![player_state(1, Vec3Net::new(5.0, 0.0, 0.0))],
        };
        let mut runtime = ClientRuntime {
            client_id: Some(1),
            snapshot: Some(current_snapshot.clone()),
            predicted_local: Some(PlayerController::from_player_state(
                &current_snapshot.players[0],
            )),
            ..default()
        };

        runtime.apply_message(ServerMessage::Snapshot(WorldSnapshot {
            tick: 4,
            players: vec![player_state(1, Vec3Net::ZERO)],
        }));

        let predicted = runtime.predicted_local.expect("prediction should exist");
        assert_eq!(predicted.position, Vec3Net::new(5.0, 0.0, 0.0));
        assert_eq!(runtime.snapshot.expect("snapshot should exist").tick, 5);
    }

    #[test]
    fn correction_updates_health_without_realigning_local_prediction() {
        let mut correction = player_state(1, Vec3Net::ZERO);
        correction.health = 42.0;
        let mut runtime = ClientRuntime {
            client_id: Some(1),
            predicted_local: Some(PlayerController::from_player_state(&player_state(
                1,
                Vec3Net::new(5.0, 0.0, 0.0),
            ))),
            ..default()
        };

        runtime.apply_message(ServerMessage::Correction(correction));

        let predicted = runtime.predicted_local.expect("prediction should exist");
        assert_eq!(predicted.position, Vec3Net::new(5.0, 0.0, 0.0));
        assert_eq!(predicted.health, 42.0);
    }

    #[test]
    fn client_messages_keep_recent_entries_only() {
        let mut runtime = ClientRuntime::default();

        for index in 0..MAX_CLIENT_LOG_MESSAGES + 5 {
            runtime.push_system_message(format!("message {index}"));
        }

        assert_eq!(runtime.messages.len(), MAX_CLIENT_LOG_MESSAGES);
        assert_eq!(runtime.messages[0].text, "message 5");
        assert_eq!(
            runtime
                .messages
                .last()
                .expect("last message should exist")
                .text,
            format!("message {}", MAX_CLIENT_LOG_MESSAGES + 4)
        );
    }

    #[test]
    fn menu_and_confirmation_defaults_match_initial_ui_state() {
        let menu = MenuState::default();
        assert_eq!(menu.screen, Screen::MainMenu);
        assert_eq!(menu.new_world_name, "New World");
        assert_eq!(menu.multiplayer_addr, "127.0.0.1:7777");
        assert!(!menu.pause_open);
        assert!(!menu.chat_open);
        assert!(menu.confirmation.is_none());

        let world_id = Uuid::new_v4();
        let dialog = ConfirmationDialog::delete_world(world_id, "Old Save");
        assert_eq!(dialog.title, "Delete World");
        assert!(dialog.body.contains("Old Save"));
        assert!(matches!(
            dialog.action,
            ConfirmationAction::DeleteWorld { world_id: id } if id == world_id
        ));
        assert!(!dialog.closing);
        assert!(!dialog.confirmed);
    }

    #[test]
    fn apply_message_handles_welcome_chat_events_and_rejections() {
        let snapshot = WorldSnapshot {
            tick: 9,
            players: vec![player_state(1, Vec3Net::new(1.0, 2.0, 3.0))],
        };
        let mut runtime = ClientRuntime::default();

        runtime.apply_message(ServerMessage::Welcome {
            client_id: 1,
            map: crate::world::MapType::Test,
            world: crate::world::WorldData::test_world(),
            is_admin: true,
            snapshot,
        });
        runtime.apply_message(ServerMessage::PlayerEvent(PlayerEvent::Joined {
            client_id: 2,
            name: "Friend".to_owned(),
        }));
        runtime.apply_message(ServerMessage::PlayerEvent(PlayerEvent::Left {
            client_id: 2,
            name: "Friend".to_owned(),
        }));
        runtime.apply_message(ServerMessage::Chat(ChatMessage {
            from: "Friend".to_owned(),
            text: "hello".to_owned(),
        }));
        runtime.apply_message(ServerMessage::AuthRejected {
            reason: "bad token".to_owned(),
        });
        runtime.apply_message(ServerMessage::Heartbeat);

        assert_eq!(runtime.client_id, Some(1));
        assert!(runtime.is_admin);
        assert!(runtime.world.is_some());
        assert_eq!(
            runtime.local_view().expect("local view").position,
            Vec3Net::new(1.0, 2.0, 3.0)
        );
        assert!(
            runtime
                .messages
                .iter()
                .any(|message| message.text == "Friend joined")
        );
        assert!(
            runtime
                .messages
                .iter()
                .any(|message| message.text == "Friend left")
        );
        assert!(runtime.messages.iter().any(|message| {
            matches!(message.kind, ClientLogKind::Chat { ref from } if from == "Friend")
        }));
        assert!(
            runtime
                .messages
                .iter()
                .any(|message| message.text.contains("auth rejected"))
        );
    }

    #[test]
    fn local_view_falls_back_to_snapshot_when_prediction_is_missing() {
        let mut runtime = ClientRuntime {
            client_id: Some(1),
            snapshot: Some(WorldSnapshot {
                tick: 1,
                players: vec![player_state(1, Vec3Net::new(4.0, 0.0, 0.0))],
            }),
            ..Default::default()
        };

        assert_eq!(
            runtime.local_player().expect("local player").position,
            Vec3Net::new(4.0, 0.0, 0.0)
        );
        assert_eq!(
            runtime.local_view().expect("local view").position,
            Vec3Net::new(4.0, 0.0, 0.0)
        );

        runtime.client_id = Some(99);
        assert!(runtime.local_player().is_none());
        assert!(runtime.local_view().is_none());
    }

    #[test]
    fn correction_and_snapshot_ignore_non_matching_players() {
        let mut runtime = ClientRuntime {
            client_id: Some(1),
            predicted_local: Some(PlayerController::from_player_state(&player_state(
                1,
                Vec3Net::new(5.0, 0.0, 0.0),
            ))),
            ..Default::default()
        };
        let mut other_player = player_state(2, Vec3Net::ZERO);
        other_player.health = 5.0;

        runtime.apply_message(ServerMessage::Correction(other_player));

        assert_eq!(
            runtime
                .predicted_local
                .as_ref()
                .expect("prediction should exist")
                .health,
            MAX_HEALTH
        );

        runtime.client_id = None;
        runtime.seed_local_prediction_from_snapshot(
            &WorldSnapshot {
                tick: 1,
                players: vec![player_state(1, Vec3Net::ZERO)],
            },
            true,
        );
        assert!(runtime.predicted_local.is_some());
    }
}
