use super::*;
use crate::{
    protocol::{ServerMessage, ToastKind},
    resources::{IRON_NODE_ID, SULFUR_NODE_ID},
};

#[test]
fn spawn_ore_command_requires_admin_and_warns_otherwise() {
    let mut server = server();
    // Connect a second player as a non-admin (host steam_id is 1, this one is 2).
    let (_, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            2,
            "Guest".to_owned(),
            crate::steam::offline_auth_token(2),
        )
        .expect("guest should connect");
    let guest_id = server
        .snapshot()
        .players
        .iter()
        .find(|player| player.steam_id == 2)
        .map(|player| player.client_id)
        .expect("guest client id");
    let before = server.snapshot().resource_nodes.len();

    let envelopes = server.receive(
        guest_id,
        ClientMessage::Command {
            text: "spawn-ore coal 5".to_owned(),
        },
    );

    let warning = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::Toast(payload) => Some(payload.clone()),
            _ => None,
        })
        .expect("non-admin should still get a warning toast");
    assert_eq!(warning.kind, ToastKind::Warning);
    assert!(warning.text.to_ascii_lowercase().contains("admin"));
    assert_eq!(
        server.snapshot().resource_nodes.len(),
        before,
        "non-admin command must not mutate the world"
    );
}

#[test]
fn spawn_ore_command_inserts_a_new_node_for_an_admin() {
    let mut server = server();
    let host_id = connect_host(&mut server);
    let before = server.snapshot().resource_nodes.len();
    let known_ids: std::collections::HashSet<u64> = server
        .snapshot()
        .resource_nodes
        .iter()
        .map(|node| node.id)
        .collect();

    let envelopes = server.receive(
        host_id,
        ClientMessage::Command {
            text: "spawn-ore iron 12".to_owned(),
        },
    );

    let success = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::Toast(payload) if payload.kind == ToastKind::Success => {
                Some(payload.clone())
            }
            _ => None,
        })
        .expect("admin spawn should get a success toast");
    assert!(success.text.to_ascii_lowercase().contains("iron"));

    let after_nodes = server.snapshot().resource_nodes;
    assert_eq!(after_nodes.len(), before + 1);
    let new_node = after_nodes
        .iter()
        .find(|node| !known_ids.contains(&node.id))
        .expect("a new node id should have been allocated");
    assert_eq!(new_node.definition_id, IRON_NODE_ID);
}

#[test]
fn spawn_ore_command_defaults_to_a_random_ore_when_type_is_omitted() {
    let mut server = server();
    let host_id = connect_host(&mut server);
    let before: std::collections::HashSet<u64> = server
        .snapshot()
        .resource_nodes
        .iter()
        .map(|node| node.id)
        .collect();

    let envelopes = server.receive(
        host_id,
        ClientMessage::Command {
            text: "spawn-ore".to_owned(),
        },
    );

    let success = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::Toast(payload) if payload.kind == ToastKind::Success => {
                Some(payload.clone())
            }
            _ => None,
        })
        .expect("admin spawn should get a success toast even without args");
    let _ = success;

    let new_node = server
        .snapshot()
        .resource_nodes
        .into_iter()
        .find(|node| !before.contains(&node.id))
        .expect("a new node should have been spawned");
    assert!(matches!(
        new_node.definition_id.as_str(),
        COAL_NODE_ID | IRON_NODE_ID | SULFUR_NODE_ID
    ));
}

#[test]
fn help_command_replies_as_server_chat_only_to_issuer() {
    let mut server = server();
    let host_id = connect_host(&mut server);

    let envelopes = server.receive(
        host_id,
        ClientMessage::Command {
            text: "help".to_owned(),
        },
    );

    // Every help line should be a directed Chat message from "Server",
    // never a Toast and never a Broadcast — the help reply must not pollute
    // other players' chat logs.
    assert!(!envelopes.is_empty(), "/help should produce chat lines");
    let chat_lines: Vec<_> = envelopes
        .iter()
        .filter_map(|envelope| match (&envelope.target, &envelope.message) {
            (super::DeliveryTarget::Client(target), ServerMessage::Chat(chat))
                if *target == host_id =>
            {
                Some(chat.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(chat_lines.len(), envelopes.len());
    assert!(chat_lines.iter().all(|chat| chat.from == "Server"));
    assert!(chat_lines.iter().any(|chat| chat.text.contains("/help")));
    assert!(
        chat_lines
            .iter()
            .any(|chat| chat.text.contains("/spawn-ore"))
    );
}

#[test]
fn help_marks_spawn_ore_as_admin_only_for_non_admins() {
    let mut server = server();
    // Guest is steam_id 2; the host (steam_id 1) is the singleplayer admin.
    let _ = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            2,
            "Guest".to_owned(),
            crate::steam::offline_auth_token(2),
        )
        .expect("guest should connect");
    let guest_id = server
        .snapshot()
        .players
        .iter()
        .find(|player| player.steam_id == 2)
        .map(|player| player.client_id)
        .expect("guest client id");

    let envelopes = server.receive(
        guest_id,
        ClientMessage::Command {
            text: "help".to_owned(),
        },
    );

    let spawn_ore_line = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::Chat(chat) if chat.text.contains("/spawn-ore") => {
                Some(chat.text.clone())
            }
            _ => None,
        })
        .expect("help should list /spawn-ore for non-admins too");
    assert!(
        spawn_ore_line.to_ascii_lowercase().contains("admin"),
        "non-admin help should signal that /spawn-ore is admin-only, got: {spawn_ore_line}"
    );
}

#[test]
fn unknown_command_returns_a_warning_without_world_mutation() {
    let mut server = server();
    let host_id = connect_host(&mut server);
    let before = server.snapshot().resource_nodes.len();

    let envelopes = server.receive(
        host_id,
        ClientMessage::Command {
            text: "nope what".to_owned(),
        },
    );

    let toast = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::Toast(payload) => Some(payload.clone()),
            _ => None,
        })
        .expect("unknown command should still produce a toast");
    assert_eq!(toast.kind, ToastKind::Warning);
    assert!(toast.text.contains("unknown"));
    assert_eq!(server.snapshot().resource_nodes.len(), before);
}
