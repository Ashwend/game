use bevy::prelude::default;
use uuid::Uuid;

use super::{
    backdrop::{MENU_BACKDROP_BLUR_WARMUP_SECONDS, MENU_BACKDROP_FADE_SECONDS},
    menu::DEFAULT_MULTIPLAYER_ADDR,
    runtime::MAX_CLIENT_LOG_MESSAGES,
    *,
};
use crate::{
    controller::PlayerController,
    protocol::{
        ChatMessage, ClientId, MAX_HEALTH, PlayerEvent, PlayerState, ServerMessage, Vec3Net,
        WorldSnapshot,
    },
    world::{MapType, ProceduralMapSize, WorldData},
    world_time::{WorldTime, WorldTimeSnapshot},
};

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
        chat_bubble: None,
        inventory: None,
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
            dropped_items: Vec::new(),
            resource_nodes: Vec::new(),
        },
        true,
    );

    let predicted = runtime.predicted_local.expect("prediction should exist");
    assert_eq!(predicted.position, Vec3Net::new(2.0, 0.0, 0.0));
    assert_eq!(runtime.input_sequence, 7);
}

#[test]
fn snapshots_do_not_overwrite_existing_local_prediction() {
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
        dropped_items: Vec::new(),
        resource_nodes: Vec::new(),
    }));

    let predicted = runtime.predicted_local.expect("prediction should exist");
    assert_eq!(predicted.position, Vec3Net::new(5.0, 0.0, 0.0));
    assert_eq!(runtime.snapshot.expect("snapshot should exist").tick, 1);
}

#[test]
fn snapshots_do_not_seed_local_prediction_after_welcome() {
    let mut runtime = ClientRuntime {
        client_id: Some(1),
        ..default()
    };

    runtime.apply_message(ServerMessage::Snapshot(WorldSnapshot {
        tick: 1,
        players: vec![player_state(1, Vec3Net::new(5.0, 0.0, 0.0))],
        dropped_items: Vec::new(),
        resource_nodes: Vec::new(),
    }));

    assert!(runtime.predicted_local.is_none());
    assert_eq!(
        runtime.local_view().expect("snapshot fallback").position,
        Vec3Net::new(5.0, 0.0, 0.0)
    );
}

#[test]
fn stale_snapshots_are_ignored() {
    let current_snapshot = WorldSnapshot {
        tick: 5,
        players: vec![player_state(1, Vec3Net::new(5.0, 0.0, 0.0))],
        dropped_items: Vec::new(),
        resource_nodes: Vec::new(),
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
        dropped_items: Vec::new(),
        resource_nodes: Vec::new(),
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
fn push_error_message_only_appends_to_chat_log() {
    // `push_error_message` is the chat-log-only side of the pair; the
    // toast surface is driven by `ClientErrorToast` events sent at the
    // call site. This test pins down that "log only" guarantee so future
    // refactors don't accidentally reintroduce a hidden side-channel.
    let mut runtime = ClientRuntime::default();
    runtime.push_error_message("network error: timeout");
    runtime.push_error_message("chat send failed");

    assert_eq!(runtime.messages.len(), 2);
    assert!(
        runtime
            .messages
            .iter()
            .all(|entry| matches!(entry.kind, ClientLogKind::Error))
    );
}

#[test]
fn connection_lag_flag_requires_active_session_and_silence_threshold() {
    let mut runtime = ClientRuntime::default();
    runtime.tick_connection_silence(10.0);
    assert!(
        !runtime.connection_is_lagging(),
        "no active session means there is no link to flag as lagging"
    );

    // Even past the warning threshold, without an active session the flag
    // stays false — `connection_is_lagging` gates on `session.is_some()`.
    let runtime = ClientRuntime {
        connection: super::connection::ConnectionWatch::with_silence(
            super::CONNECTION_LAG_WARNING_SECONDS + 1.0,
        ),
        ..default()
    };
    assert!(!runtime.connection_is_lagging());
}

#[test]
fn applying_any_server_message_resets_the_silence_counter() {
    let mut runtime = ClientRuntime {
        connection: super::connection::ConnectionWatch::with_silence(5.0),
        ..default()
    };
    runtime.apply_message(ServerMessage::Heartbeat);
    // After receive the lag flag is cleared even with an active session.
    assert!(!runtime.connection.is_lagging(true));
}

#[test]
fn client_messages_keep_recent_entries_only() {
    let mut runtime = ClientRuntime::default();

    for index in 0..MAX_CLIENT_LOG_MESSAGES + 5 {
        runtime.push_system_message(format!("message {index}"));
    }

    assert_eq!(runtime.messages.len(), MAX_CLIENT_LOG_MESSAGES);
    assert_eq!(runtime.messages.front().expect("first").text, "message 5");
    assert_eq!(
        runtime
            .messages
            .back()
            .expect("last message should exist")
            .text,
        format!("message {}", MAX_CLIENT_LOG_MESSAGES + 4)
    );
}

#[test]
fn shutdown_tasks_drain_completed_results() {
    let mut tasks = SessionShutdownTasks::default();
    tasks.push_finished_for_test(Ok(()));
    tasks.push_finished_for_test(Err("save failed".to_owned()));

    let mut results = Vec::new();
    for _ in 0..20 {
        results.extend(tasks.drain_finished());
        if results.len() == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    assert_eq!(tasks.pending_len(), 0);
    assert_eq!(results.len(), 2);
    assert!(results.iter().any(Result::is_ok));
    assert!(
        results
            .iter()
            .any(|result| matches!(result, Err(error) if error == "save failed"))
    );
}

#[test]
fn menu_and_confirmation_defaults_match_initial_ui_state() {
    let menu = MenuState::default();
    assert_eq!(menu.screen, Screen::MainMenu);
    assert!(menu.create_world.is_none());
    assert!(menu.edit_world.is_none());
    assert!(menu.direct_connect.is_none());
    assert!(menu.world_start.is_none());
    assert_eq!(menu.multiplayer_addr, DEFAULT_MULTIPLAYER_ADDR);
    assert!(!menu.pause_open);
    assert!(!menu.pause_options_open);
    assert!(!menu.inventory_open);
    assert!(!menu.chat_open);
    assert!(menu.confirmation.is_none());
    assert!(menu.notice.is_none());
    assert!(!menu.quit_requested);

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
fn direct_connect_dialog_separates_address_and_port() {
    let dialog = DirectConnectDialog::new(DEFAULT_MULTIPLAYER_ADDR);
    assert_eq!(dialog.host, "46.224.101.205");
    assert_eq!(dialog.port, "7777");
    assert!(dialog.error.is_none());
    assert!(!dialog.closing);
    assert!(!dialog.is_connecting());

    let fallback = DirectConnectDialog::new("example.invalid");
    assert_eq!(fallback.host, "example.invalid");
    assert_eq!(fallback.port, "7777");
}

#[test]
fn create_world_dialog_builds_selected_maps() {
    let mut dialog = CreateWorldDialog::default();

    assert_eq!(dialog.name, "New World");
    assert_eq!(dialog.selected_map().expect("test map"), MapType::Test);

    dialog.map_kind = CreateWorldMapKind::Procedural;
    dialog.procedural_size = ProceduralMapSize::Large;
    dialog.seed = "42".to_owned();
    assert_eq!(
        dialog.selected_map().expect("procedural map"),
        MapType::Procedural {
            seed: 42,
            size: ProceduralMapSize::Large,
        }
    );

    dialog.seed = "not a number".to_owned();
    assert!(dialog.selected_map().is_err());
    dialog.refresh_seed();
    assert!(dialog.selected_map().is_ok());
}

#[test]
fn menu_backdrop_visibility_covers_until_blur_warms() {
    let mut visibility = MenuBackdropVisibility::default();

    let warmup_alpha =
        visibility.cover_alpha(Screen::MainMenu, MENU_BACKDROP_BLUR_WARMUP_SECONDS * 0.5);
    assert_eq!(warmup_alpha, u8::MAX);

    let fading_alpha = visibility.cover_alpha(
        Screen::MainMenu,
        MENU_BACKDROP_BLUR_WARMUP_SECONDS * 0.5 + MENU_BACKDROP_FADE_SECONDS * 0.5,
    );
    assert!(fading_alpha > 0);
    assert!(fading_alpha < u8::MAX);

    let visible_alpha = visibility.cover_alpha(Screen::MainMenu, MENU_BACKDROP_FADE_SECONDS);
    assert_eq!(visible_alpha, 0);
}

#[test]
fn menu_backdrop_visibility_resets_when_reentering_menu() {
    let mut visibility = MenuBackdropVisibility::default();

    assert_eq!(
        visibility.cover_alpha(
            Screen::MainMenu,
            MENU_BACKDROP_BLUR_WARMUP_SECONDS + MENU_BACKDROP_FADE_SECONDS,
        ),
        0
    );
    assert_eq!(visibility.cover_alpha(Screen::InGame, 0.1), 0);
    assert_eq!(visibility.cover_alpha(Screen::MainMenu, 0.1), u8::MAX);
}

#[test]
fn apply_message_handles_welcome_chat_events_and_rejections() {
    let snapshot = WorldSnapshot {
        tick: 9,
        players: vec![player_state(1, Vec3Net::new(1.0, 2.0, 3.0))],
        dropped_items: Vec::new(),
        resource_nodes: Vec::new(),
    };
    let mut runtime = ClientRuntime::default();

    runtime.apply_message(ServerMessage::Welcome {
        client_id: 1,
        map: MapType::Test,
        world: WorldData::test_world(),
        is_admin: true,
        snapshot,
        world_time: WorldTimeSnapshot::from_time(&WorldTime::default(), 9),
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
fn kicked_message_clears_session_state_and_logs_reason() {
    let mut runtime = ClientRuntime {
        client_id: Some(1),
        is_admin: true,
        world: Some(WorldData::test_world()),
        snapshot: Some(WorldSnapshot::default()),
        predicted_local: Some(PlayerController::spawn()),
        ..Default::default()
    };

    runtime.apply_message(ServerMessage::Kicked {
        reason: "Server restart".to_owned(),
    });

    assert!(runtime.client_id.is_none());
    assert!(!runtime.is_admin);
    assert!(runtime.world.is_none());
    assert!(runtime.snapshot.is_none());
    assert!(runtime.predicted_local.is_none());
    assert!(
        runtime
            .messages
            .iter()
            .any(|message| message.text == "disconnected: Server restart")
    );
}

#[test]
fn local_view_falls_back_to_snapshot_when_prediction_is_missing() {
    let mut server_player = player_state(1, Vec3Net::new(4.0, 0.0, 0.0));
    server_player.yaw = 0.75;
    server_player.pitch = -0.25;
    let mut runtime = ClientRuntime {
        client_id: Some(1),
        snapshot: Some(WorldSnapshot {
            tick: 1,
            players: vec![server_player],
            dropped_items: Vec::new(),
            resource_nodes: Vec::new(),
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
    let local_view = runtime.local_view().expect("local view");
    assert_eq!(local_view.yaw, 0.75);
    assert_eq!(local_view.pitch, -0.25);

    runtime.client_id = Some(99);
    assert!(runtime.local_player().is_none());
    assert!(runtime.local_view().is_none());
}

#[test]
fn local_view_uses_predicted_orientation_with_predicted_position() {
    let mut predicted_player = player_state(1, Vec3Net::new(5.0, 0.0, 0.0));
    predicted_player.yaw = 1.25;
    predicted_player.pitch = -0.35;
    let mut snapshot_player = player_state(1, Vec3Net::new(1.0, 0.0, 0.0));
    snapshot_player.yaw = -0.5;
    snapshot_player.pitch = 0.2;
    let runtime = ClientRuntime {
        client_id: Some(1),
        predicted_local: Some(PlayerController::from_player_state(&predicted_player)),
        snapshot: Some(WorldSnapshot {
            tick: 1,
            players: vec![snapshot_player],
            dropped_items: Vec::new(),
            resource_nodes: Vec::new(),
        }),
        ..Default::default()
    };

    let local_view = runtime.local_view().expect("local view");
    assert_eq!(local_view.position, Vec3Net::new(5.0, 0.0, 0.0));
    assert_eq!(local_view.yaw, 1.25);
    assert_eq!(local_view.pitch, -0.35);
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
            dropped_items: Vec::new(),
            resource_nodes: Vec::new(),
        },
        true,
    );
    assert!(runtime.predicted_local.is_some());
}
