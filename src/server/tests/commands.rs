use super::*;
use crate::{
    protocol::{ServerMessage, ToastKind},
    resources::{IRON_NODE_ID, PINE_TREE_LARGE_NODE_ID},
};

#[test]
fn spawn_command_requires_admin_and_warns_otherwise() {
    let mut server = server();
    // Connect a second player as a non-admin (host account_id is 1, this one is 2).
    let (_, _) = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            2,
            "Guest".to_owned(),
            String::new(),
        )
        .expect("guest should connect");
    let guest_id = server
        .players_iter()
        .find(|player| player.account_id == 2)
        .map(|player| player.client_id)
        .expect("guest client id");
    let before = server.resource_nodes_iter().count();

    let envelopes = server.receive(
        guest_id,
        ClientMessage::Command {
            text: "spawn coal 5".to_owned(),
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
        server.resource_nodes_iter().count(),
        before,
        "non-admin command must not mutate the world"
    );
}

#[test]
fn spawn_command_inserts_a_new_node_for_an_admin() {
    let mut server = server();
    let host_id = connect_host(&mut server);
    let before = server.resource_nodes_iter().count();
    let known_ids: std::collections::HashSet<u64> =
        server.resource_nodes_iter().map(|(id, _)| *id).collect();

    let envelopes = server.receive(
        host_id,
        ClientMessage::Command {
            text: "spawn iron 12".to_owned(),
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

    let after_nodes: Vec<_> = server.resource_nodes_iter().collect();
    assert_eq!(after_nodes.len(), before + 1);
    let new_node = after_nodes
        .iter()
        .find(|(id, _)| !known_ids.contains(id))
        .expect("a new node id should have been allocated");
    assert_eq!(new_node.1.definition_id, IRON_NODE_ID);
}

#[test]
fn spawn_command_handles_non_ore_kinds_and_warns_when_kind_is_missing() {
    let mut server = server();
    let host_id = connect_host(&mut server);
    let before: std::collections::HashSet<u64> =
        server.resource_nodes_iter().map(|(id, _)| *id).collect();

    // A tree alias goes through the same registry + chunk-tracking path
    // as the ores.
    let envelopes = server.receive(
        host_id,
        ClientMessage::Command {
            text: "spawn pine-large".to_owned(),
        },
    );
    assert!(
        envelopes.iter().any(|envelope| matches!(
            &envelope.message,
            ServerMessage::Toast(payload) if payload.kind == ToastKind::Success
        )),
        "admin spawn of a tree should succeed"
    );
    let nodes_after: Vec<_> = server.resource_nodes_iter().collect();
    let new_node = nodes_after
        .into_iter()
        .find(|(id, _)| !before.contains(id))
        .expect("a new node should have been spawned");
    assert_eq!(new_node.1.definition_id, PINE_TREE_LARGE_NODE_ID);

    // Without a kind there is nothing sensible to spawn; the issuer gets
    // a usage warning instead.
    let envelopes = server.receive(
        host_id,
        ClientMessage::Command {
            text: "spawn".to_owned(),
        },
    );
    assert!(
        envelopes.iter().any(|envelope| matches!(
            &envelope.message,
            ServerMessage::Toast(payload) if payload.kind == ToastKind::Warning
        )),
        "missing kind should warn with usage"
    );
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
    // never a Toast and never a Broadcast, the help reply must not pollute
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
    assert!(chat_lines.iter().any(|chat| chat.text.contains("/spawn")));
}

#[test]
fn help_marks_spawn_as_admin_only_for_non_admins() {
    let mut server = server();
    // Guest is account_id 2; the host (account_id 1) is the singleplayer admin.
    let _ = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            2,
            "Guest".to_owned(),
            String::new(),
        )
        .expect("guest should connect");
    let guest_id = server
        .players_iter()
        .find(|player| player.account_id == 2)
        .map(|player| player.client_id)
        .expect("guest client id");

    let envelopes = server.receive(
        guest_id,
        ClientMessage::Command {
            text: "help".to_owned(),
        },
    );

    let spawn_line = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::Chat(chat) if chat.text.contains("/spawn") => Some(chat.text.clone()),
            _ => None,
        })
        .expect("help should list /spawn for non-admins too");
    assert!(
        spawn_line.to_ascii_lowercase().contains("admin"),
        "non-admin help should signal that /spawn is admin-only, got: {spawn_line}"
    );
}

#[test]
fn unknown_command_returns_a_warning_without_world_mutation() {
    let mut server = server();
    let host_id = connect_host(&mut server);
    let before = server.resource_nodes_iter().count();

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
    assert_eq!(server.resource_nodes_iter().count(), before);
}
