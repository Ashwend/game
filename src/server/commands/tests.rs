use super::world::{SmallRng, parse_node_token};
use super::*;
use crate::{
    auth::AuthMode,
    items::{
        BASIC_HATCHET_ID, BASIC_PICKAXE_ID, CLOTH_ID, COAL_ID, CRUDE_FURNACE_ID, FIBER_ID,
        GUNPOWDER_ID, IRON_BAR_ID, IRON_ORE_ID, METEORITE_ALLOY_ID, PADDED_HOOD_ID,
        PADDED_LEGGINGS_ID, PADDED_TUNIC_ID, PADDED_WRAPS_ID, PLANT_TWINE_ID, SALVAGED_FITTINGS_ID,
        STONE_ID, SULFUR_ID, SULFUR_ORE_ID, WOOD_ID, WORKBENCH_T1_ID, stack_limit,
    },
    protocol::{GAME_VERSION, PROTOCOL_VERSION, Vec3Net},
    resources::{
        BIRCH_TREE_LARGE_NODE_ID, BRANCH_PILE_NODE_ID, COAL_NODE_ID, IRON_NODE_ID,
        PINE_TREE_NODE_ID, PINE_TREE_SMALL_NODE_ID, SULFUR_NODE_ID,
    },
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
fn time_speed_applies_clamped_multiplier_and_rejects_garbage() {
    let (mut server, client) = server_with_host(Some(1));
    // `/time-speed` now owns the day/night cycle speed (was `/speed`).
    let ok = server.apply_command(client, "/time-speed 4".to_owned());
    assert!(has_toast(&ok, ToastKind::Success));
    assert_eq!(server.world_time.multiplier, 4.0);

    // Non-finite/non-number rejected without mutating.
    let bad = server.apply_command(client, "/time-speed fast".to_owned());
    assert!(has_toast(&bad, ToastKind::Warning));
    assert_eq!(server.world_time.multiplier, 4.0);

    // Negative below MIN_MULTIPLIER rejected.
    let neg = server.apply_command(client, "/time-speed -1".to_owned());
    assert!(has_toast(&neg, ToastKind::Warning));
    assert_eq!(server.world_time.multiplier, 4.0);
}

#[test]
fn speed_sets_the_players_run_speed_multiplier() {
    let (mut server, client) = server_with_host(Some(1));
    // `/speed` is now the run-speed cheat for the issuing player.
    let ok = server.apply_command(client, "/speed 2.5".to_owned());
    assert!(has_toast(&ok, ToastKind::Success));
    assert_eq!(server.clients[&client].run_speed_multiplier, 2.5);
    // It does NOT touch the day/night cycle speed.
    assert_eq!(server.world_time.multiplier, 1.0);

    // Out-of-range clamps (no freeze, no absurd values).
    server.apply_command(client, "/speed 0".to_owned());
    assert!(server.clients[&client].run_speed_multiplier >= 0.1);
    server.apply_command(client, "/speed 9999".to_owned());
    assert!(server.clients[&client].run_speed_multiplier <= 20.0);

    // Garbage rejected without mutating.
    server.apply_command(client, "/speed 3".to_owned());
    let bad = server.apply_command(client, "/speed zoom".to_owned());
    assert!(has_toast(&bad, ToastKind::Warning));
    assert_eq!(server.clients[&client].run_speed_multiplier, 3.0);
}

#[test]
fn speed_is_admin_only() {
    let (mut server, client) = server_with_host(None);
    let denied = server.apply_command(client, "/speed 5".to_owned());
    assert!(has_toast(&denied, ToastKind::Warning));
    assert_eq!(server.clients[&client].run_speed_multiplier, 1.0);
}

#[test]
fn knockback_scale_sets_the_global_factor_and_clamps() {
    let (mut server, client) = server_with_host(Some(1));
    // Fresh server is neutral.
    assert_eq!(server.knockback_scale, 1.0);

    // A valid factor is applied and acknowledged with a success toast.
    let ok = server.apply_command(client, "/knockback-scale 1.5".to_owned());
    assert!(has_toast(&ok, ToastKind::Success));
    assert_eq!(server.knockback_scale, 1.5);

    // Out-of-range clamps at both ends (0 is allowed as the floor).
    server.apply_command(client, "/knockback-scale -3".to_owned());
    assert_eq!(server.knockback_scale, 0.0);
    server.apply_command(client, "/knockback-scale 9999".to_owned());
    assert_eq!(server.knockback_scale, 5.0);

    // Reset to neutral.
    server.apply_command(client, "/knockback-scale 1".to_owned());
    assert_eq!(server.knockback_scale, 1.0);
}

#[test]
fn knockback_scale_rejects_garbage_without_mutating() {
    let (mut server, client) = server_with_host(Some(1));
    server.apply_command(client, "/knockback-scale 2".to_owned());
    assert_eq!(server.knockback_scale, 2.0);

    // Unparseable arg is rejected and leaves the factor untouched.
    let bad = server.apply_command(client, "/knockback-scale wobble".to_owned());
    assert!(has_toast(&bad, ToastKind::Warning));
    assert_eq!(server.knockback_scale, 2.0);

    // A non-finite value is rejected too.
    let inf = server.apply_command(client, "/knockback-scale inf".to_owned());
    assert!(has_toast(&inf, ToastKind::Warning));
    assert_eq!(server.knockback_scale, 2.0);

    // Missing arg warns with usage.
    let missing = server.apply_command(client, "/knockback-scale".to_owned());
    assert!(has_toast(&missing, ToastKind::Warning));
    assert_eq!(server.knockback_scale, 2.0);
}

#[test]
fn knockback_scale_is_admin_only() {
    let (mut server, client) = server_with_host(None);
    let denied = server.apply_command(client, "/knockback-scale 3".to_owned());
    assert!(has_toast(&denied, ToastKind::Warning));
    // Non-admin rejection leaves the global factor at its neutral default.
    assert_eq!(server.knockback_scale, 1.0);
}

#[test]
fn spawn_admin_inserts_a_node_directly_in_front_of_the_player() {
    let (mut server, client) = server_with_host(Some(1));
    {
        let c = server.clients.get_mut(&client).unwrap();
        c.controller.position = Vec3Net::new(10.0, 0.0, -4.0);
        // Yaw 0 looks down -Z (forward = (-sin, 0, -cos) = (0, 0, -1)).
        c.controller.yaw = 0.0;
    }
    let known_ids: std::collections::HashSet<u64> = server.resource_nodes.keys().copied().collect();

    let out = server.apply_command(client, "/spawn iron 6".to_owned());
    assert!(has_toast(&out, ToastKind::Success));

    let (_, node) = server
        .resource_nodes
        .iter()
        .find(|(id, _)| !known_ids.contains(id))
        .expect("spawn should insert exactly one new node");
    assert_eq!(node.definition_id, IRON_NODE_ID);
    assert!((node.position.x - 10.0).abs() < 1e-3);
    assert!((node.position.z - (-10.0)).abs() < 1e-3, "6m down -Z");
    assert_eq!(node.position.y, 0.0);
}

#[test]
fn spawn_uses_default_distance_when_omitted_and_clamps_tiny_values() {
    let (mut server, client) = server_with_host(Some(1));
    {
        let c = server.clients.get_mut(&client).unwrap();
        c.controller.position = Vec3Net::ZERO;
        c.controller.yaw = 0.0;
    }

    // The generated world already contains pines and branch piles, so
    // identify the spawned node by id diff, not by definition lookup.
    let known_ids: std::collections::HashSet<u64> = server.resource_nodes.keys().copied().collect();
    let out = server.apply_command(client, "/spawn pine".to_owned());
    assert!(has_toast(&out, ToastKind::Success));
    let (default_id, default_node) = server
        .resource_nodes
        .iter()
        .find(|(id, _)| !known_ids.contains(id))
        .expect("pine should spawn");
    assert_eq!(default_node.definition_id, PINE_TREE_NODE_ID);
    assert!(
        (default_node.position.z - (-4.0)).abs() < 1e-3,
        "default 4m"
    );
    let default_id = *default_id;

    // A sub-minimum distance is clamped up instead of placing the node
    // inside the player's collision radius.
    let out = server.apply_command(client, "/spawn sticks 0.1".to_owned());
    assert!(has_toast(&out, ToastKind::Success));
    let (_, clamped) = server
        .resource_nodes
        .iter()
        .find(|(id, _)| !known_ids.contains(id) && **id != default_id)
        .expect("branch pile should spawn");
    assert_eq!(clamped.definition_id, BRANCH_PILE_NODE_ID);
    assert!(
        (clamped.position.z - (-1.75)).abs() < 1e-3,
        "clamped to min"
    );
}

#[test]
fn spawn_rejects_bad_kind_missing_kind_and_nonpositive_distance() {
    let (mut server, client) = server_with_host(Some(1));
    let before = server.resource_nodes.len();
    let bad_arg = server.apply_command(client, "/spawn granite".to_owned());
    assert!(has_toast(&bad_arg, ToastKind::Warning));

    let no_kind = server.apply_command(client, "/spawn".to_owned());
    assert!(has_toast(&no_kind, ToastKind::Warning));

    let bad_distance = server.apply_command(client, "/spawn iron -2".to_owned());
    assert!(has_toast(&bad_distance, ToastKind::Warning));
    assert_eq!(
        server.resource_nodes.len(),
        before,
        "no node should be inserted on rejection"
    );
}

#[test]
fn spawn_rejected_for_non_admin() {
    let (mut server, client) = server_with_host(None);
    let before = server.resource_nodes.len();
    let out = server.apply_command(client, "/spawn iron".to_owned());
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
fn parse_node_token_accepts_aliases_separators_and_registry_ids() {
    assert_eq!(parse_node_token("coal"), Some(COAL_NODE_ID));
    assert_eq!(parse_node_token("IRON"), Some(IRON_NODE_ID));
    assert_eq!(parse_node_token("sulphur"), Some(SULFUR_NODE_ID));
    assert_eq!(parse_node_token("pine"), Some(PINE_TREE_NODE_ID));
    assert_eq!(
        parse_node_token("birch-large"),
        Some(BIRCH_TREE_LARGE_NODE_ID)
    );
    assert_eq!(parse_node_token("sticks"), Some(BRANCH_PILE_NODE_ID));
    assert_eq!(
        parse_node_token("pine_tree_small"),
        Some(PINE_TREE_SMALL_NODE_ID)
    );
    assert_eq!(parse_node_token("granite"), None);
}

#[test]
fn small_rng_emits_changing_values() {
    let mut rng = SmallRng { state: 0xCAFE };
    let first = rng.next_u32();
    let second = rng.next_u32();
    assert_ne!(first, second);
}

#[test]
fn give_grants_a_specific_resource_in_registry_stacks() {
    let (mut server, client) = server_with_host(Some(1));
    let out = server.apply_command(client, "/give stone".to_owned());
    assert!(has_toast(&out, ToastKind::Success));
    let total =
        crate::inventory::count_items_in_inventory(&server.clients[&client].inventory, STONE_ID);
    assert_eq!(total, 1000, "default count is 1000");
}

#[test]
fn give_all_grants_every_base_resource_at_the_requested_count() {
    let (mut server, client) = server_with_host(Some(1));
    let out = server.apply_command(client, "/give all 50".to_owned());
    assert!(has_toast(&out, ToastKind::Success));
    for item_id in [WOOD_ID, STONE_ID, FIBER_ID, IRON_ORE_ID] {
        let total =
            crate::inventory::count_items_in_inventory(&server.clients[&client].inventory, item_id);
        assert_eq!(total, 50, "{item_id} should be granted");
    }
}

#[test]
fn give_rejects_unknown_items_bad_counts_and_non_admins() {
    let (mut server, client) = server_with_host(Some(1));
    let out = server.apply_command(client, "/give frobnium".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));
    let out = server.apply_command(client, "/give stone 0".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));
    let out = server.apply_command(client, "/give".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));

    let (mut server, client) = server_with_host(None);
    let out = server.apply_command(client, "/give stone".to_owned());
    assert!(has_toast(&out, ToastKind::Warning));
    let total =
        crate::inventory::count_items_in_inventory(&server.clients[&client].inventory, STONE_ID);
    assert_eq!(total, 0, "non-admins get nothing");
}

#[test]
fn test_kit_command_grants_full_kit_and_routes_equipables_to_actionbar() {
    use crate::server::test_support::{connect_named, server};
    let mut server = server();
    let client_id = connect_named(&mut server, "Tester");

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

    // The first equipables land in the actionbar (nine slots, filled in the
    // EQUIPABLES order): the four tools come first, so they are always on the
    // bar. With the four weapons added the actionbar now fills before
    // the deployables, so later equipables (workbench, furnace, ...) fall back to
    // the inventory grid, which the kit does on purpose.
    let actionbar_ids: Vec<_> = client
        .inventory
        .actionbar_slots
        .iter()
        .filter_map(|slot| slot.as_ref().map(|s| s.item_id.as_ref().to_owned()))
        .collect();
    for required in [BASIC_HATCHET_ID, BASIC_PICKAXE_ID] {
        assert!(
            actionbar_ids.iter().any(|id| id == required),
            "actionbar should contain the tool {required}, got {actionbar_ids:?}",
        );
    }
    // Every equipable is granted somewhere (actionbar first, inventory fallback),
    // including the ones that overflow the nine actionbar slots.
    for required in [WORKBENCH_T1_ID, CRUDE_FURNACE_ID] {
        assert!(
            crate::inventory::count_items_in_inventory(&client.inventory, required) >= 1,
            "the kit should grant {required} to the actionbar or the inventory",
        );
    }

    // Every resource type sits in the main inventory at a full stack
    // capped to its registry limit: the kit targets 100 of each, so a
    // 200-cap material lands at 100, a 50-cap material (cloth, fittings)
    // at 50, and meteorite (20-cap) at 20. This also proves the Ember
    // Age intermediates and rare exploration resources reach the bag.
    for resource in [
        WOOD_ID,
        STONE_ID,
        COAL_ID,
        IRON_ORE_ID,
        SULFUR_ORE_ID,
        SULFUR_ID,
        GUNPOWDER_ID,
        FIBER_ID,
        CLOTH_ID,
        PLANT_TWINE_ID,
        IRON_BAR_ID,
        METEORITE_ALLOY_ID,
        SALVAGED_FITTINGS_ID,
    ] {
        let expected = stack_limit(resource).expect("registered resource").min(100);
        let stack = client
            .inventory
            .inventory_slots
            .iter()
            .filter_map(|slot| slot.as_ref())
            .find(|stack| stack.item_id.as_ref() == resource)
            .unwrap_or_else(|| panic!("inventory should contain {resource}"));
        assert_eq!(
            stack.quantity, expected,
            "{resource} should be granted as {expected} (capped to its stack limit)"
        );
        assert!(
            stack.quantity <= stack_limit(resource).unwrap(),
            "{resource} kit stack must not exceed its registry cap"
        );
    }

    // The full padded armor set is granted too, one of each piece, so a
    // tester can equip a set and see mitigation without crafting. Pieces
    // land in the actionbar first and spill to the inventory, so search
    // both containers.
    for piece in [
        PADDED_HOOD_ID,
        PADDED_TUNIC_ID,
        PADDED_LEGGINGS_ID,
        PADDED_WRAPS_ID,
    ] {
        let found = client
            .inventory
            .actionbar_slots
            .iter()
            .chain(client.inventory.inventory_slots.iter())
            .filter_map(|slot| slot.as_ref())
            .any(|stack| stack.item_id.as_ref() == piece);
        assert!(found, "kit should contain padded piece {piece}");
    }
}

#[test]
fn test_kit_command_refused_for_non_admin() {
    // Singleplayer host is admin; spin up a server with NO host so
    // the connecting client is a plain non-admin (account id 7).
    let (mut server, client_id) = server_with_host(None);

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
