use super::world::{MIN_SPAWN_ORE_DISTANCE, SmallRng, parse_ore_token, random_position_around};
use super::*;
use crate::{
    auth::AuthMode,
    items::{
        BASIC_HATCHET_ID, BASIC_PICKAXE_ID, COAL_ID, CRUDE_FURNACE_ID, FIBER_ID, IRON_BAR_ID,
        IRON_ORE_ID, PLANT_TWINE_ID, STONE_ID, SULFUR_ORE_ID, WOOD_ID, WORKBENCH_T1_ID,
    },
    protocol::{GAME_VERSION, PROTOCOL_VERSION, Vec3Net},
    resources::{COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID},
    save::WorldSave,
    server::ServerSettings,
};

/// Spin up a server. `host` controls whether the connecting client
/// becomes the implicit singleplayer admin.
fn server_with_host(host: Option<u64>) -> (GameServer, ClientId) {
    let mut server = GameServer::new(
        WorldSave::new("Test", host),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: host,
        },
    );
    let account_id = host.unwrap_or(7);
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            account_id,
            "Tester".to_owned(),
            String::new(),
        )
        .expect("connect ok");
    (server, client_id)
}

fn has_toast(envelopes: &[ServerEnvelope], kind: ToastKind) -> bool {
    envelopes.iter().any(|e| {
        matches!(&e.message, ServerMessage::Toast(t) if std::mem::discriminant(&t.kind) == std::mem::discriminant(&kind))
    })
}

#[test]
fn empty_command_warns() {
    let (mut server, client) = server_with_host(Some(1));
    let out = server.apply_command(client, "/   ".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));
}

#[test]
fn unknown_command_warns() {
    let (mut server, client) = server_with_host(Some(1));
    let out = server.apply_command(client, "/frobnicate".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));
}

#[test]
fn whisper_delivers_to_target_and_echoes_to_sender_without_broadcast() {
    let (mut server, sender) = server_with_host(Some(1));
    let (recipient, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            2,
            "Bob".to_owned(),
            String::new(),
        )
        .expect("connect ok");

    let out = server.apply_command(sender, "/w Bob hello there".to_owned());

    let to_recipient = out.iter().any(|e| {
        matches!(
            (&e.target, &e.message),
            (DeliveryTarget::Client(id), ServerMessage::Chat(c))
                if *id == recipient && c.text == "hello there"
        )
    });
    let to_sender = out.iter().any(|e| {
        matches!(
            (&e.target, &e.message),
            (DeliveryTarget::Client(id), ServerMessage::Chat(c))
                if *id == sender && c.text == "hello there"
        )
    });
    assert!(to_recipient, "recipient should receive the whisper");
    assert!(to_sender, "sender should get an echo");
    assert!(
        !out.iter()
            .any(|e| matches!(e.target, DeliveryTarget::Broadcast)),
        "a whisper must never broadcast"
    );
}

#[test]
fn whisper_matches_names_case_insensitively() {
    let (mut server, sender) = server_with_host(Some(1));
    let (recipient, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            2,
            "Bob".to_owned(),
            String::new(),
        )
        .expect("connect ok");

    let out = server.apply_command(sender, "/w bOb yo".to_owned());
    assert!(out.iter().any(|e| {
        matches!(
            (&e.target, &e.message),
            (DeliveryTarget::Client(id), ServerMessage::Chat(_)) if *id == recipient
        )
    }));
}

#[test]
fn whisper_to_unknown_player_warns() {
    let (mut server, sender) = server_with_host(Some(1));
    let out = server.apply_command(sender, "/w Nobody hi".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));
}

#[test]
fn whisper_without_a_message_warns() {
    let (mut server, sender) = server_with_host(Some(1));
    let _ = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            2,
            "Bob".to_owned(),
            String::new(),
        )
        .expect("connect ok");
    let out = server.apply_command(sender, "/w Bob   ".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));
}

#[test]
fn help_lists_commands_as_chat_for_admin_and_non_admin() {
    // Admin sees the unlocked descriptions; non-admin sees "admin
    // only" tags. Both get the list as Chat (not toast).
    let (mut server, admin) = server_with_host(Some(1));
    let admin_lines = server.apply_command(admin, "/help".to_owned());
    assert!(
        admin_lines
            .iter()
            .all(|e| matches!(&e.message, ServerMessage::Chat(_)))
    );
    let admin_text: String = admin_lines
        .iter()
        .filter_map(|e| match &e.message {
            ServerMessage::Chat(c) => Some(c.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(admin_text.contains("/test-kit: grant"));

    let (mut server2, non_admin) = server_with_host(None);
    let lines = server2.apply_command(non_admin, "/help".to_owned());
    let text: String = lines
        .iter()
        .filter_map(|e| match &e.message {
            ServerMessage::Chat(c) => Some(c.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("admin only"),
        "non-admin help should flag gated commands"
    );
}

#[test]
fn set_time_admin_success_and_parse_error() {
    let (mut server, client) = server_with_host(Some(1));
    let ok = server.apply_command(client, "/time 06:30".to_owned());
    assert!(has_toast(&ok, ToastKind::Success));
    // The broadcast WorldTime envelope rides along with the toast.
    assert!(
        ok.iter()
            .any(|e| matches!(&e.message, ServerMessage::WorldTime(_)))
    );

    let bad = server.apply_command(client, "/time half-past".to_owned());
    assert!(has_toast(&bad, ToastKind::Warning));
    assert!(
        !bad.iter()
            .any(|e| matches!(&e.message, ServerMessage::WorldTime(_)))
    );
}

#[test]
fn set_time_missing_arg_warns() {
    let (mut server, client) = server_with_host(Some(1));
    let out = server.apply_command(client, "/time".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));
}

#[test]
fn set_time_rejected_for_non_admin() {
    let (mut server, client) = server_with_host(None);
    let out = server.apply_command(client, "/time 06:30".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));
    assert!(
        !out.iter()
            .any(|e| matches!(&e.message, ServerMessage::WorldTime(_)))
    );
}

#[test]
fn set_speed_applies_clamped_multiplier_and_rejects_garbage() {
    let (mut server, client) = server_with_host(Some(1));
    let ok = server.apply_command(client, "/speed 4".to_owned());
    assert!(has_toast(&ok, ToastKind::Success));
    assert_eq!(server.world_time.multiplier, 4.0);

    // Non-finite/non-number rejected without mutating.
    let bad = server.apply_command(client, "/speed fast".to_owned());
    assert!(has_toast(&bad, ToastKind::Warning));
    assert_eq!(server.world_time.multiplier, 4.0);

    // Negative below MIN_MULTIPLIER rejected.
    let neg = server.apply_command(client, "/speed -1".to_owned());
    assert!(has_toast(&neg, ToastKind::Warning));
    assert_eq!(server.world_time.multiplier, 4.0);
}

#[test]
fn spawn_ore_admin_inserts_a_node_within_radius() {
    let (mut server, client) = server_with_host(Some(1));
    let before = server.resource_nodes.len();
    let out = server.apply_command(client, "/spawn-ore iron 10".to_owned());
    assert!(has_toast(&out, ToastKind::Success));
    assert_eq!(
        server.resource_nodes.len(),
        before + 1,
        "spawn-ore should insert exactly one node"
    );
}

#[test]
fn spawn_ore_rejects_bad_argument_and_nonpositive_radius() {
    let (mut server, client) = server_with_host(Some(1));
    let before = server.resource_nodes.len();
    let bad_arg = server.apply_command(client, "/spawn-ore granite".to_owned());
    assert!(has_toast(&bad_arg, ToastKind::Warning));

    let bad_radius = server.apply_command(client, "/spawn-ore iron -2".to_owned());
    assert!(has_toast(&bad_radius, ToastKind::Warning));
    assert_eq!(
        server.resource_nodes.len(),
        before,
        "no node should be inserted on rejection"
    );
}

#[test]
fn spawn_ore_rejected_for_non_admin() {
    let (mut server, client) = server_with_host(None);
    let before = server.resource_nodes.len();
    let out = server.apply_command(client, "/spawn-ore iron".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));
    assert_eq!(
        server.resource_nodes.len(),
        before,
        "non-admin must not spawn a node"
    );
}

#[test]
fn teleport_all_with_no_other_players_reports_none() {
    let (mut server, client) = server_with_host(Some(1));
    let out = server.apply_command(client, "/tp".to_owned());
    // Only a toast, no Correction envelopes when alone.
    assert!(has_toast(&out, ToastKind::Success));
    assert!(
        !out.iter()
            .any(|e| matches!(&e.message, ServerMessage::Correction(_)))
    );
}

#[test]
fn teleport_all_moves_other_players_and_sends_corrections() {
    let (mut server, admin) = server_with_host(Some(1));
    // Position the admin somewhere distinctive.
    {
        let c = server.clients.get_mut(&admin).unwrap();
        c.controller.position = Vec3Net::new(12.0, 0.0, -7.0);
    }
    // Connect a second, non-host player far away.
    let (other, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            2,
            "Other".to_owned(),
            String::new(),
        )
        .expect("second connect ok");
    {
        let c = server.clients.get_mut(&other).unwrap();
        c.controller.position = Vec3Net::new(-50.0, 0.0, 50.0);
        c.controller.velocity = Vec3Net::new(3.0, 0.0, 3.0);
    }

    let out = server.apply_command(admin, "/tp".to_owned());
    assert!(has_toast(&out, ToastKind::Success));
    let correction_for_other = out.iter().any(|e| {
        matches!(
            (&e.target, &e.message),
            (DeliveryTarget::Client(id), ServerMessage::Correction(_)) if *id == other
        )
    });
    assert!(
        correction_for_other,
        "a Correction must be sent to the teleported player"
    );

    let moved = &server.clients[&other].controller;
    assert_eq!(moved.position.x, 12.0);
    assert_eq!(moved.position.z, -7.0);
    assert_eq!(
        moved.velocity,
        Vec3Net::ZERO,
        "teleport zeroes inbound momentum"
    );
}

#[test]
fn parse_ore_token_accepts_canonical_and_alternate_spellings() {
    assert_eq!(parse_ore_token("coal"), Some(COAL_NODE_ID));
    assert_eq!(parse_ore_token("IRON"), Some(IRON_NODE_ID));
    assert_eq!(parse_ore_token("sulphur"), Some(SULFUR_NODE_ID));
    assert_eq!(parse_ore_token("granite"), None);
}

#[test]
fn random_position_lands_inside_the_radius_and_outside_the_inner_ring() {
    let mut rng = SmallRng { state: 0x1234_5678 };
    let center = Vec3Net::new(10.0, 0.0, -3.0);
    for _ in 0..200 {
        let position = random_position_around(center, 12.0, &mut rng);
        let dx = position.x - center.x;
        let dz = position.z - center.z;
        let r = (dx * dx + dz * dz).sqrt();
        assert!(r <= 12.0 + 1e-3, "{r} should stay inside the outer ring");
        assert!(
            r >= MIN_SPAWN_ORE_DISTANCE.min(12.0 * 0.5) - 1e-3,
            "{r} should not land inside the inner cull"
        );
        assert_eq!(position.y, 0.0);
    }
}

#[test]
fn small_rng_emits_changing_values() {
    let mut rng = SmallRng { state: 0xCAFE };
    let first = rng.next_u32();
    let second = rng.next_u32();
    assert_ne!(first, second);
}

#[test]
fn test_kit_command_grants_full_kit_and_routes_equipables_to_actionbar() {
    use crate::{
        auth::AuthMode,
        protocol::{GAME_VERSION, PROTOCOL_VERSION},
        save::WorldSave,
        server::ServerSettings,
    };
    let mut server = crate::server::GameServer::new(
        WorldSave::new("Test", Some(1)),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: Some(1),
        },
    );
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            1,
            "Tester".to_owned(),
            String::new(),
        )
        .expect("connect ok");

    // The singleplayer host gets admin status implicitly, so the
    // command should succeed on this freshly-connected client.
    let envelopes = server.apply_command(client_id, "/test-kit".to_owned());
    assert!(
        envelopes.iter().any(|envelope| matches!(
            &envelope.message,
            ServerMessage::Toast(toast) if matches!(toast.kind, ToastKind::Success)
        )),
        "test-kit should reply with a success toast"
    );

    let client = server
        .clients
        .get(&client_id)
        .expect("client still connected");

    // Tools + structures landed in the actionbar.
    let actionbar_ids: Vec<_> = client
        .inventory
        .actionbar_slots
        .iter()
        .filter_map(|slot| slot.as_ref().map(|s| s.item_id.as_ref().to_owned()))
        .collect();
    for required in [
        BASIC_HATCHET_ID,
        BASIC_PICKAXE_ID,
        WORKBENCH_T1_ID,
        CRUDE_FURNACE_ID,
    ] {
        assert!(
            actionbar_ids.iter().any(|id| id == required),
            "actionbar should contain {required}, got {actionbar_ids:?}",
        );
    }

    // Every resource type sits in the main inventory at the kit
    // quantity. Iron bar is capped at 100, others at 200, so 100
    // is always intact.
    for resource in [
        WOOD_ID,
        STONE_ID,
        COAL_ID,
        IRON_ORE_ID,
        SULFUR_ORE_ID,
        FIBER_ID,
        PLANT_TWINE_ID,
        IRON_BAR_ID,
    ] {
        let stack = client
            .inventory
            .inventory_slots
            .iter()
            .filter_map(|slot| slot.as_ref())
            .find(|stack| stack.item_id.as_ref() == resource)
            .unwrap_or_else(|| panic!("inventory should contain {resource}"));
        assert_eq!(stack.quantity, 100, "{resource} should be granted as 100");
    }
}

#[test]
fn test_kit_command_refused_for_non_admin() {
    use crate::{
        auth::AuthMode,
        protocol::{GAME_VERSION, PROTOCOL_VERSION},
        save::WorldSave,
        server::ServerSettings,
    };
    // Singleplayer host is admin; spin up a server with NO host so
    // the connecting client is a plain non-admin.
    let mut server = crate::server::GameServer::new(
        WorldSave::new("Test", None),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: None,
        },
    );
    let (client_id, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            7,
            "Tester".to_owned(),
            String::new(),
        )
        .expect("connect ok");

    let envelopes = server.apply_command(client_id, "/test-kit".to_owned());
    assert!(
        envelopes.iter().any(|envelope| matches!(
            &envelope.message,
            ServerMessage::Toast(toast) if matches!(toast.kind, ToastKind::Warning)
        )),
        "non-admin should be rejected with a warning toast",
    );

    // Confirm no inventory mutation happened.
    let client = server.clients.get(&client_id).unwrap();
    let granted = client
        .inventory
        .inventory_slots
        .iter()
        .chain(client.inventory.actionbar_slots.iter())
        .any(|slot| slot.is_some());
    assert!(!granted, "non-admin must not have received any items");
}
