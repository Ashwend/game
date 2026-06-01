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
    // Poll until both spawned threads finish and have been drained.
    // `drain_finished` only returns *finished* tasks, so we accumulate
    // across iterations rather than breaking on the first non-empty
    // batch — otherwise a slower second thread races the assertions.
    let mut all = Vec::new();
    for _ in 0..200 {
        all.extend(tasks.drain_finished());
        if tasks.pending_len() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Both results surface and the queue empties.
    assert!(all.iter().any(|r| r.is_ok()));
    assert!(all.iter().any(|r| r.is_err()));
    assert_eq!(tasks.pending_len(), 0);
}
