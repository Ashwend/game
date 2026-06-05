use super::*;

#[test]
fn singleplayer_host_is_admin() {
    let mut server = server();
    let (client_id, envelopes) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            1,
            "Host".to_owned(),
            String::new(),
        )
        .expect("host should connect");

    assert_eq!(client_id, 1);
    assert!(matches!(
        &envelopes[0].message,
        ServerMessage::Welcome { is_admin: true, .. }
    ));
}

#[test]
fn workos_mode_rejects_a_connection_it_cannot_verify() {
    // `NoAuth` trusts the claim, so there's nothing to reject there, the real
    // gate is `Workos` mode. A Workos-mode server with no configured verifier
    // can't validate the token, so the handshake is refused rather than
    // admitting the claimed identity.
    let mut server = GameServer::new(
        WorldSave::new("Test", Some(1)),
        ServerSettings {
            auth_mode: AuthMode::Workos,
            singleplayer_host: Some(1),
        },
    );
    assert!(
        server
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                2,
                "Bad".to_owned(),
                "not.a.jwt".to_owned(),
            )
            .is_err()
    );
}

#[test]
fn rejects_mismatched_client_versions() {
    let mut server = server();
    let error = server
        .connect(
            PROTOCOL_VERSION,
            Some("0.1.0".to_owned()),
            1,
            "Host".to_owned(),
            String::new(),
        )
        .expect_err("version mismatch should reject auth");

    assert!(error.to_string().contains("version mismatch"));
}

#[test]
fn version_mismatch_is_a_typed_rejection_carrying_both_sides() {
    let mut server = server();
    // A bumped protocol is a version mismatch just like a different build.
    let error = server
        .connect(
            PROTOCOL_VERSION + 1,
            Some("0.0.1".to_owned()),
            1,
            "Host".to_owned(),
            String::new(),
        )
        .expect_err("a mismatched protocol must be rejected");

    let mismatch = error
        .downcast_ref::<crate::server::VersionMismatchRejection>()
        .expect("rejection must be typed so routing answers with ServerMessage::VersionMismatch");
    assert_eq!(mismatch.server_version, GAME_VERSION);
    assert_eq!(mismatch.server_protocol, PROTOCOL_VERSION);
    assert_eq!(mismatch.client_version.as_deref(), Some("0.0.1"));
    assert_eq!(mismatch.client_protocol, PROTOCOL_VERSION + 1);
}

#[test]
fn chat_is_sanitized_and_broadcast_by_server() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let envelopes = server.receive(
        client_id,
        ClientMessage::Chat {
            text: "  hello server  ".to_owned(),
        },
    );

    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].target, DeliveryTarget::Broadcast);
    assert!(matches!(
        &envelopes[0].message,
        ServerMessage::Chat(ChatMessage { from, text })
            if from == "Host" && text == "hello server"
    ));
}

#[test]
fn chat_populates_speaker_bubble_for_broadcast_window() {
    use crate::protocol::CHAT_BUBBLE_DURATION_SECONDS;

    let mut server = server();
    let client_id = connect_host(&mut server);

    let _ = server.receive(
        client_id,
        ClientMessage::Chat {
            text: "hi there".to_owned(),
        },
    );

    let speaker = server
        .players_iter()
        .find(|player| player.client_id == client_id)
        .expect("speaker should be in players_iter");
    assert_eq!(speaker.public.chat_bubble.as_deref(), Some("hi there"));

    let dt = 1.0 / SERVER_TICK_RATE_HZ;
    let ticks_to_expire = (CHAT_BUBBLE_DURATION_SECONDS * SERVER_TICK_RATE_HZ) as u64 + 1;
    for _ in 0..ticks_to_expire {
        server.tick(dt);
    }

    let speaker = server
        .players_iter()
        .find(|player| player.client_id == client_id)
        .expect("speaker should still be present");
    assert!(
        speaker.public.chat_bubble.is_none(),
        "bubble should auto-clear after the broadcast window"
    );
}

#[test]
fn fresh_spawn_is_random_in_bounds_and_clear_of_colliders() {
    use crate::controller::{BlockGrid, player_overlaps_world};
    use crate::world::PlayableBounds;

    // Raw connect (not the origin-pinning `connect_host` helper) so we observe
    // the real random initial spawn.
    let mut server = server();
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            7,
            "Wanderer".to_owned(),
            String::new(),
        )
        .expect("connect ok");
    let pos = server.clients[&client_id].controller.position;

    // In-bounds and on the floor.
    let bounds = PlayableBounds::from_dims(server.chunk_manager.dims());
    assert!(
        bounds.contains(pos.x, pos.z),
        "spawn {pos:?} should land inside the playable bounds"
    );
    assert_eq!(pos.y, 0.0, "spawn sits on the flat floor");

    // Rebuild the same collider set the picker used (world blocks + node and
    // deployable colliders) and confirm the spawn isn't inside any of it.
    let mut extras: Vec<_> = server
        .resource_nodes
        .values()
        .filter_map(crate::resources::resource_node_collider)
        .collect();
    extras.extend(
        server
            .deployed_entities
            .values()
            .filter_map(|e| e.resolved_collider()),
    );
    let grid = BlockGrid::build_with_extras(&server.world, &extras);
    assert!(
        !player_overlaps_world(pos, &grid),
        "spawn {pos:?} must not land inside a collider"
    );
}

#[test]
fn auto_save_warns_then_flags_a_save_on_schedule() {
    // Pick an interval just past the 30s warning window so both the heads-up
    // and the save fire within a short, fast loop.
    let warning_ticks = (SERVER_TICK_RATE_HZ as u64) * 30;
    let interval = warning_ticks + 4;
    let mut server = server().with_auto_save(interval);

    let dt = 1.0 / SERVER_TICK_RATE_HZ;
    let mut warned = false;
    let mut announced_saving = false;
    let mut saw_pending = false;

    for _ in 0..interval {
        let envelopes = server.tick(dt);
        for envelope in &envelopes {
            if let ServerMessage::Chat(chat) = &envelope.message {
                if chat.text.contains("30 seconds") {
                    warned = true;
                }
                if chat.text.contains("Auto-saving") {
                    announced_saving = true;
                }
            }
        }
        // The host drains this each tick to perform the write; here we just
        // confirm it's raised exactly when the save announcement goes out.
        if server.take_auto_save_pending() {
            saw_pending = true;
            assert!(
                announced_saving,
                "the save flag must not precede the saving announcement"
            );
        }
    }

    assert!(warned, "a 30-second heads-up should be announced");
    assert!(announced_saving, "the save start should be announced");
    assert!(
        saw_pending,
        "the host should be signalled to write the save"
    );
}

#[test]
fn auto_save_is_disabled_by_default() {
    // Loopback/singleplayer never calls `with_auto_save`, so the schedule must
    // stay dormant: no announcements, no pending flag, ever.
    let mut server = server();
    let dt = 1.0 / SERVER_TICK_RATE_HZ;
    for _ in 0..((SERVER_TICK_RATE_HZ as u64) * 31) {
        let envelopes = server.tick(dt);
        assert!(
            !envelopes.iter().any(|e| matches!(
                &e.message,
                ServerMessage::Chat(chat) if chat.text.contains("save")
            )),
            "auto-save must stay silent when disabled"
        );
        assert!(!server.take_auto_save_pending());
    }
}

#[test]
fn two_fresh_players_spawn_apart() {
    // Both connect before any tick advances, so they share an RNG seed; the
    // per-player distance check must still split them onto different spots
    // rather than stacking them.
    let mut server = server();
    let (a, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            11,
            "A".to_owned(),
            String::new(),
        )
        .expect("connect a");
    let (b, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            22,
            "B".to_owned(),
            String::new(),
        )
        .expect("connect b");

    let pa = server.clients[&a].controller.position;
    let pb = server.clients[&b].controller.position;
    let dx = pa.x - pb.x;
    let dz = pa.z - pb.z;
    let distance = (dx * dx + dz * dz).sqrt();
    assert!(
        distance >= crate::game_balance::RESPAWN_MIN_DISTANCE_M,
        "two fresh spawns should be at least the min spawn distance apart, got {distance}"
    );
}

#[test]
fn empty_chat_is_ignored_by_server() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let envelopes = server.receive(
        client_id,
        ClientMessage::Chat {
            text: "   ".to_owned(),
        },
    );

    assert!(envelopes.is_empty());
}

#[test]
fn server_announcements_are_broadcast_as_chat() {
    let server = server();

    let envelopes = server.announce("  restart soon  ");

    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].target, DeliveryTarget::Broadcast);
    assert!(matches!(
        &envelopes[0].message,
        ServerMessage::Chat(ChatMessage { from, text })
            if from == "Server" && text == "restart soon"
    ));
}

#[test]
fn silent_clients_become_sleeping_bodies_after_timeout() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let mut envelopes = Vec::new();

    for _ in 0..=CLIENT_STALE_TIMEOUT_TICKS {
        envelopes = server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    assert!(matches!(
        envelopes.iter().find_map(|envelope| match &envelope.message {
            ServerMessage::PlayerEvent(event) => Some(event),
            _ => None,
        }),
        Some(PlayerEvent::Left { client_id: left_id, name })
            if *left_id == client_id && name == "Host"
    ));
    assert!(
        envelopes.iter().any(|envelope| matches!(
            envelope.target,
            DeliveryTarget::Disconnect(target_id) if target_id == client_id
        )),
        "stale-client eviction must emit a transport-level Disconnect so Lightyear tears the session down"
    );
    // The body does not leave the world, it becomes a sleeping body.
    let client = server
        .clients
        .get(&client_id)
        .expect("a timed-out client's body stays in the world as a sleeper");
    assert!(!client.online, "the body should now be asleep");
    assert_eq!(server.players_iter().count(), 1);

    // Reconnecting the same account wakes the body in place (same id reused).
    let (woken_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            1,
            "Host".to_owned(),
            String::new(),
        )
        .expect("reconnect should wake the sleeper");
    assert_eq!(woken_id, client_id, "wake reuses the sleeping body's id");
    assert!(server.clients.get(&client_id).is_some_and(|c| c.online));
}

#[test]
fn disconnect_sleeps_the_body_and_reconnect_wakes_it_in_place() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    // Stand the body at a known spot with an item so we can prove the woken
    // body resumes exactly where it slept.
    {
        let client = server.clients.get_mut(&client_id).unwrap();
        client.controller.position = Vec3Net::new(5.0, 0.0, -3.0);
        client.inventory.inventory_slots[0] = Some(ItemStack::new(COAL_ID, 7));
    }

    let envelopes = server.disconnect(client_id);
    // The body sleeps: still in the world, mirrored as sleeping, but offline.
    let client = server.clients.get(&client_id).expect("body persists");
    assert!(!client.online, "the body should be asleep");
    let view = server
        .players_iter()
        .find(|view| view.client_id == client_id)
        .expect("the sleeping body is still mirrored");
    assert!(view.sleeping.0, "the mirror flags it as sleeping");
    assert!(envelopes.iter().any(|envelope| matches!(
        &envelope.message,
        ServerMessage::PlayerEvent(PlayerEvent::Left { client_id: id, .. }) if *id == client_id
    )));

    // A repeat disconnect (the netcode `Disconnected` event after the clean
    // quit) is a no-op rather than a second announcement.
    assert!(server.disconnect(client_id).is_empty());

    // Reconnecting the same account wakes the body in place.
    let (woken, env) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            1,
            "Host".to_owned(),
            String::new(),
        )
        .expect("reconnect wakes the sleeper");
    assert_eq!(woken, client_id, "wake reuses the same body id");
    let client = server.clients.get(&client_id).unwrap();
    assert!(client.online);
    assert!(
        (client.controller.position.x - 5.0).abs() < f32::EPSILON,
        "woke where it slept"
    );
    assert!(
        client.inventory.inventory_slots[0].is_some(),
        "kept its inventory through the sleep"
    );
    assert!(
        env.iter()
            .any(|envelope| matches!(&envelope.message, ServerMessage::Welcome { .. })),
        "the wake delivers a Welcome"
    );
}

#[test]
fn heartbeat_keeps_client_online_until_it_stops() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    for _ in 0..CLIENT_STALE_TIMEOUT_TICKS {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }
    server.receive(client_id, ClientMessage::Heartbeat);
    for _ in 0..CLIENT_STALE_TIMEOUT_TICKS {
        server.tick(1.0 / SERVER_TICK_RATE_HZ);
    }

    assert!(
        server.clients.get(&client_id).is_some_and(|c| c.online),
        "the heartbeat kept the session online"
    );
    // Now it goes silent: the next sweep puts the body to sleep. It stays in
    // the world (still one mirrored body) but is no longer online.
    server.tick(1.0 / SERVER_TICK_RATE_HZ);
    let client = server
        .clients
        .get(&client_id)
        .expect("body persists as a sleeper");
    assert!(!client.online);
    assert_eq!(server.players_iter().count(), 1);
}

#[test]
fn reconnecting_at_zero_health_respawns_alive() {
    // A player who disconnects while dead persists at 0 HP (lifecycle isn't
    // saved). On reconnect they must come back alive at full health rather
    // than as a 0-HP "zombie" the combat path refuses to hit.
    let mut server = server();
    let client_id = connect_host(&mut server);

    // Simulate the dead state that disconnect snapshots into the store.
    server
        .clients
        .get_mut(&client_id)
        .expect("connected client")
        .controller
        .health = 0.0;
    let _ = server.disconnect(client_id);

    let (reconnected, _envelopes) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            1,
            "Host".to_owned(),
            String::new(),
        )
        .expect("returning host should reconnect");

    let client = server
        .clients
        .get(&reconnected)
        .expect("reconnected client exists");
    assert_eq!(
        client.controller.health,
        crate::protocol::MAX_HEALTH,
        "a player who died should not return as a 0-HP zombie"
    );
    assert!(
        client.lifecycle.is_alive(),
        "a respawned reconnect must be alive"
    );
}

#[test]
fn ping_is_echoed_as_pong_with_the_same_timestamp() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let out = server.receive(
        client_id,
        ClientMessage::Ping {
            client_time_ms: 12_345,
            rtt_ms: 42,
        },
    );

    assert!(out.iter().any(|e| {
        matches!(
            (&e.target, &e.message),
            (DeliveryTarget::Client(id), ServerMessage::Pong { client_time_ms })
                if *id == client_id && *client_time_ms == 12_345
        )
    }));
}

#[test]
fn reported_ping_surfaces_in_the_player_list_broadcast() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    let _ = server.receive(
        client_id,
        ClientMessage::Ping {
            client_time_ms: 0,
            rtt_ms: 73,
        },
    );

    // Tick across at least one roster-broadcast interval and capture it.
    let mut roster = None;
    for _ in 0..(SERVER_TICK_RATE_HZ as usize + 2) {
        for envelope in server.tick(1.0 / SERVER_TICK_RATE_HZ) {
            if let ServerMessage::PlayerList(entries) = envelope.message {
                roster = Some(entries);
            }
        }
    }

    let entries = roster.expect("a PlayerList broadcast should have fired within a second");
    let me = entries
        .iter()
        .find(|entry| entry.client_id == client_id)
        .expect("the connected player should be in the roster");
    assert_eq!(me.ping_ms, 73);
    assert_eq!(me.name, "Host");
}

#[test]
fn kick_all_sends_reason_before_disconnects() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let envelopes = server.kick_all("Server restart");

    let kicked_index = envelopes
        .iter()
        .position(|envelope| {
            matches!(
                envelope,
                ServerEnvelope {
                    target: DeliveryTarget::Client(target_id),
                    message: ServerMessage::Kicked { reason },
                } if *target_id == client_id && reason == "Server restart"
            )
        })
        .expect("Kicked envelope should be emitted for the kicked client");
    let disconnect_index = envelopes
        .iter()
        .position(|envelope| {
            matches!(
                envelope.target,
                DeliveryTarget::Disconnect(target_id) if target_id == client_id
            )
        })
        .expect("DeliveryTarget::Disconnect envelope should be emitted so the host layer can tear down the transport session");
    assert!(
        kicked_index < disconnect_index,
        "Kicked envelope must precede the transport-level Disconnect so the client sees the reason before the connection drops"
    );
    assert!(envelopes.iter().any(|envelope| {
        matches!(
            &envelope.message,
            ServerMessage::PlayerEvent(PlayerEvent::Left { client_id: left_id, .. })
                if *left_id == client_id
        )
    }));
    // A kick tears down the live session but leaves the body behind as a
    // sleeper (so it persists across, e.g., a server-restart kick_all).
    assert!(
        server.clients.get(&client_id).is_some_and(|c| !c.online),
        "the kicked player's body should stay as a sleeper"
    );
}

#[test]
fn reconnect_with_same_identity_takes_over_and_preserves_state() {
    let mut server = server();
    let first = connect_host(&mut server);
    equip_basic_tools(&mut server, first);

    // Move an item so we can prove the live inventory carries across the
    // takeover instead of resetting to starting state.
    server.receive(
        first,
        ClientMessage::Inventory(InventoryCommand::Move {
            from: ItemContainerSlot {
                container: crate::protocol::ItemContainer::Actionbar,
                slot: 0,
            },
            to: ItemContainerSlot {
                container: crate::protocol::ItemContainer::Actionbar,
                slot: 4,
            },
            quantity: None,
            seq: 0,
        }),
    );

    // Reconnect with the same identity before the old session has timed out.
    // This is the quick-reconnect path that used to be rejected for ~10s.
    let (second, envelopes) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            1,
            "Host".to_owned(),
            String::new(),
        )
        .expect("reconnect with the same identity should take over, not be rejected");

    assert_ne!(first, second, "takeover should issue a fresh client id");

    // The old session is torn down, a Left for the old id and a transport
    // Disconnect targeting it, and that teardown precedes the new Welcome so
    // peers and the host layer see the old session leave first.
    assert!(envelopes.iter().any(|envelope| matches!(
        &envelope.message,
        ServerMessage::PlayerEvent(PlayerEvent::Left { client_id, .. }) if *client_id == first
    )));
    let disconnect_index = envelopes
        .iter()
        .position(|envelope| {
            matches!(
                envelope.target,
                DeliveryTarget::Disconnect(id) if id == first
            )
        })
        .expect("old session must emit a transport-level Disconnect");
    let welcome_index = envelopes
        .iter()
        .position(|envelope| matches!(&envelope.message, ServerMessage::Welcome { .. }))
        .expect("new session must receive a Welcome");
    assert!(
        disconnect_index < welcome_index,
        "old session teardown must precede the new Welcome"
    );

    // Exactly one live player, and it inherited the moved inventory.
    assert_eq!(server.players_iter().count(), 1);
    let client = server.clients.get(&second).expect("new session exists");
    assert!(client.inventory.actionbar_slots[0].is_none());
    assert!(client.inventory.actionbar_slots[4].is_some());
}

#[test]
fn world_save_round_trips_player_inventory_and_position() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_basic_tools(&mut server, client_id);

    let pose = PlayerMovement {
        sequence: 1,
        position: Vec3Net::new(12.0, 4.5, -7.0),
        velocity: Vec3Net::ZERO,
        yaw: 0.75,
        pitch: -0.25,
        grounded: true,
    };
    server.receive(client_id, ClientMessage::Movement(pose));

    // Move an item onto the actionbar so we can verify inventory state
    // survives a save/load cycle.
    let envelopes = server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::Move {
            from: ItemContainerSlot {
                container: crate::protocol::ItemContainer::Actionbar,
                slot: 0,
            },
            to: ItemContainerSlot {
                container: crate::protocol::ItemContainer::Actionbar,
                slot: 4,
            },
            quantity: None,
            seq: 0,
        }),
    );
    drop(envelopes);

    let save = server.world_save();
    assert_eq!(save.state.players.len(), 1);

    let mut restored = GameServer::new(
        save,
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: Some(1),
        },
    );
    let (restored_client_id, restored_envelopes) = restored
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            1,
            "Host".to_owned(),
            String::new(),
        )
        .expect("returning host should reconnect");

    let player = restored
        .players_iter()
        .find(|player| player.client_id == restored_client_id)
        .expect("restored client should appear in the live state");
    assert!((player.public.position.x - 12.0).abs() < f32::EPSILON);
    assert!((player.public.position.y - 4.5).abs() < f32::EPSILON);
    assert!((player.public.position.z + 7.0).abs() < f32::EPSILON);
    assert!((player.public.yaw - 0.75).abs() < f32::EPSILON);

    let client = restored
        .clients
        .get(&restored_client_id)
        .expect("restored client exists");
    let inventory = &client.inventory;
    assert!(inventory.actionbar_slots[0].is_none());
    assert!(inventory.actionbar_slots[4].is_some());

    drop(restored_envelopes);
}
