use super::*;

fn coal_node(id: u64, quantity: u16) -> ResourceNodeState {
    ResourceNodeState {
        id,
        definition_id: COAL_NODE_ID.to_owned(),
        position: Vec3Net::new(0.0, 0.0, -2.2),
        yaw: 0.0,
        storage: vec![ItemStack::new(COAL_ID, quantity)],
        dead: false,
    }
}

fn look_at_test_node(server: &mut GameServer, client_id: ClientId) {
    let mut movement = movement(1, Vec3Net::ZERO);
    movement.pitch = -0.42;
    server.receive(client_id, ClientMessage::Movement(movement));
}

#[test]
fn mirror_sync_deltas_track_insert_mutate_and_remove() {
    let mut server = server();
    // The constructor seeds every generated node as dirty (so the first sync
    // spawns all mirror entities). Drain it to start from a clean slate.
    let _ = server.drain_resource_node_sync();
    let (dirty, removed) = server.drain_resource_node_sync();
    assert!(
        dirty.is_empty() && removed.is_empty(),
        "drain should clear the delta sets"
    );

    let id = 999_001;

    // Insert is recorded as dirty (→ sync spawns a mirror entity).
    server.insert_resource_node(id, coal_node(id, 5));
    let (dirty, removed) = server.drain_resource_node_sync();
    assert_eq!(dirty, vec![id]);
    assert!(removed.is_empty());

    // Mutating via the guarded accessor re-flags it dirty (→ storage diff).
    server
        .resource_node_state_mut(id)
        .expect("node present")
        .storage = vec![ItemStack::new(COAL_ID, 2)];
    let (dirty, removed) = server.drain_resource_node_sync();
    assert_eq!(dirty, vec![id]);
    assert!(removed.is_empty());

    // Removal is recorded as removed, not dirty (→ sync despawns it).
    assert!(server.remove_resource_node(id).is_some());
    let (dirty, removed) = server.drain_resource_node_sync();
    assert!(dirty.is_empty());
    assert_eq!(removed, vec![id]);

    // Insert + remove within one sync window collapses to a single removal
    // (the entity was never spawned, so the sync's despawn no-ops).
    server.insert_resource_node(id, coal_node(id, 5));
    assert!(server.remove_resource_node(id).is_some());
    let (dirty, removed) = server.drain_resource_node_sync();
    assert!(dirty.is_empty(), "removed-after-insert must not stay dirty");
    assert_eq!(removed, vec![id]);

    // A mutate-attempt on an absent node records nothing.
    assert!(server.resource_node_state_mut(id).is_none());
    let (dirty, removed) = server.drain_resource_node_sync();
    assert!(dirty.is_empty() && removed.is_empty());
}

#[test]
fn test_world_spawns_authoritative_resource_nodes() {
    let mut server = server();
    connect_host(&mut server);

    let nodes: Vec<_> = server.resource_nodes_iter().collect();

    assert!(nodes.len() >= 6);
    assert!(
        nodes
            .iter()
            .any(|(_, node)| node.definition_id == COAL_NODE_ID)
    );
}

#[test]
fn pickaxe_depletes_node_and_removes_it_from_the_world() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_basic_tools(&mut server, client_id);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 3));
    look_at_test_node(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
    );

    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );

    let client = server.clients.get(&client_id).expect("client exists");
    let inventory = &client.inventory;
    // Depleted nodes are removed from the world entirely, the chunk
    // manager schedules a fresh-position respawn 5-15 minutes later. The
    // server should no longer hold this node id.
    assert!(
        server.resource_nodes_iter().all(|(id, _)| *id != 99),
        "depleted node should be removed from the live server state"
    );
    assert!(inventory.inventory_slots.iter().any(|slot| {
        slot.as_ref()
            .is_some_and(|stack| stack.item_id.as_ref() == COAL_ID && stack.quantity == 3)
    }));
}

#[test]
fn applied_action_seq_advances_on_accepted_and_rejected_gather() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_basic_tools(&mut server, client_id);
    server.resource_nodes.clear();
    // High storage so the node never depletes during the test.
    server.resource_nodes.insert(99, coal_node(99, 100));
    look_at_test_node(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
    );

    // Accepted gather (looking at the node, correct tool) advances the mark.
    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 5,
        }),
    );
    assert_eq!(
        server.clients.get(&client_id).unwrap().applied_action_seq,
        5,
        "accepted gather advances the prediction high-water mark"
    );

    // Look away → out of range (and still inside cooldown): the gather is
    // rejected, leaving the inventory untouched, but the mark MUST still
    // advance so the client can prune and revert its optimistic overlay op.
    let inventory_before = server.clients.get(&client_id).unwrap().inventory.clone();
    let mut look_away = movement(2, Vec3Net::ZERO);
    look_away.pitch = 1.4;
    server.receive(client_id, ClientMessage::Movement(look_away));
    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 9,
        }),
    );
    let client = server.clients.get(&client_id).unwrap();
    assert_eq!(
        client.applied_action_seq, 9,
        "rejected gather still advances the high-water mark (fix #1)"
    );
    assert_eq!(
        client.inventory, inventory_before,
        "rejected gather must not change the inventory"
    );

    // A stale / duplicate (lower) seq never walks the mark backward.
    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 4,
        }),
    );
    assert_eq!(
        server.clients.get(&client_id).unwrap().applied_action_seq,
        9,
        "a stale seq must not move the high-water mark backward"
    );
}

#[test]
fn second_gather_on_removed_node_is_silently_dropped() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_basic_tools(&mut server, client_id);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 1));
    look_at_test_node(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
    );

    // First gather depletes the node, it's gone from the live map.
    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );
    assert!(!server.resource_nodes.contains_key(&99));

    // Any further gather attempts against the removed id produce nothing
    //, no toasts, no impacts, no inventory change.
    let inventory_before = {
        let client = server.clients.get(&client_id).expect("host client");
        client.inventory.clone()
    };
    let envelopes = server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );
    assert!(
        envelopes.is_empty(),
        "gather against a removed node must produce no envelopes"
    );
    {
        let client = server.clients.get(&client_id).expect("host client");
        assert_eq!(client.inventory, inventory_before);
    }
}

#[test]
fn successful_gather_emits_success_toast_to_requesting_client() {
    use crate::protocol::{ServerMessage, ToastKind};

    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_basic_tools(&mut server, client_id);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 5));
    look_at_test_node(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
    );

    let envelopes = server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );

    let toast = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::Toast(payload) => Some((envelope.target.clone(), payload.clone())),
            _ => None,
        })
        .expect("server should emit a Toast envelope on successful gather");

    assert_eq!(toast.0, super::DeliveryTarget::Client(client_id));
    assert_eq!(toast.1.kind, ToastKind::Success);
    assert!(
        toast.1.text.starts_with('+') && toast.1.text.contains("Coal"),
        "unexpected toast text: {}",
        toast.1.text
    );
}

#[test]
fn gather_into_full_inventory_emits_warning_toast_and_locks_cooldown() {
    use crate::protocol::{ServerMessage, ToastKind};

    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_basic_tools(&mut server, client_id);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 5));
    look_at_test_node(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
    );

    // Saturate every inventory slot with a non-stackable item so the coal
    // payout has nowhere to land. Keep the pickaxe equipped on slot 1.
    let client = server
        .clients
        .get_mut(&client_id)
        .expect("connected host should exist");
    for slot in client.inventory.inventory_slots.iter_mut() {
        *slot = Some(ItemStack::new(BASIC_HATCHET_ID, 1));
    }
    for (index, slot) in client.inventory.actionbar_slots.iter_mut().enumerate() {
        if index == 1 {
            continue;
        }
        *slot = Some(ItemStack::new(BASIC_HATCHET_ID, 1));
    }
    let tick_before = server.tick;

    let envelopes = server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );

    let toast = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::Toast(payload) => Some(payload.clone()),
            _ => None,
        })
        .expect("inventory-full gather should still produce a warning toast");
    assert_eq!(toast.kind, ToastKind::Warning);
    assert!(toast.text.to_ascii_lowercase().contains("full"));

    let client = server
        .clients
        .get(&client_id)
        .expect("connected host should exist");
    assert!(
        client.next_gather_tick > tick_before,
        "inventory-full gather should advance the cooldown to prevent toast spam"
    );
}

#[test]
fn failed_gather_emits_no_toast() {
    use crate::protocol::ServerMessage;

    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_basic_tools(&mut server, client_id);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 5));
    look_at_test_node(&mut server, client_id);
    // Holding the hatchet (slot 0) instead of the pickaxe means the tool does
    // not allow harvesting the coal node; no toast should fire.

    let envelopes = server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );

    assert!(
        !envelopes
            .iter()
            .any(|envelope| matches!(envelope.message, ServerMessage::Toast(_))),
        "rejected gather should not push a toast"
    );
}

#[test]
fn successful_gather_broadcasts_impact_to_peers_only() {
    use crate::protocol::{ResourceImpactKind, ServerMessage};

    let mut server = server();
    let client_id = connect_host(&mut server);
    // The impact cue is range-gated and skips the swinger, so a nearby
    // peer is the expected (only) recipient. Connected under its own
    // account id; reusing the host's would wake-reconnect that client.
    let peer = {
        let id = server
            .connect(
                crate::protocol::PROTOCOL_VERSION,
                Some(crate::protocol::GAME_VERSION.to_owned()),
                2,
                "Peer".to_owned(),
                String::new(),
            )
            .expect("peer connects")
            .0;
        server
            .clients
            .get_mut(&id)
            .expect("connected client should exist")
            .controller
            .position = Vec3Net::ZERO;
        id
    };
    equip_basic_tools(&mut server, client_id);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 5));
    look_at_test_node(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
    );

    let envelopes = server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );

    let (target, position, kind) = envelopes
        .iter()
        .find_map(|envelope| match &envelope.message {
            ServerMessage::ResourceImpact { position, kind } => {
                Some((envelope.target.clone(), *position, *kind))
            }
            _ => None,
        })
        .expect("server should emit a ResourceImpact envelope on successful gather");

    assert_eq!(
        target,
        super::DeliveryTarget::Client(peer),
        "the swinger's client already played the impact locally; the echo \
         goes to nearby peers only",
    );
    assert_eq!(kind, ResourceImpactKind::CoalOre);
    assert_eq!(position, Vec3Net::new(0.0, 0.0, -2.2));
}

#[test]
fn failed_gather_emits_no_impact_broadcast() {
    use crate::protocol::ServerMessage;

    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_basic_tools(&mut server, client_id);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 5));
    look_at_test_node(&mut server, client_id);
    // Still holding the hatchet at slot 0, wrong tool for coal.

    let envelopes = server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );

    assert!(
        !envelopes
            .iter()
            .any(|envelope| matches!(envelope.message, ServerMessage::ResourceImpact { .. })),
        "rejected gather must not broadcast an impact effect to peers",
    );
}

#[test]
fn resource_gathering_requires_matching_tool_and_server_cooldown() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    equip_basic_tools(&mut server, client_id);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 9));
    look_at_test_node(&mut server, client_id);

    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );
    assert_eq!(
        server
            .resource_nodes
            .get(&99)
            .and_then(|node| node.storage.first())
            .map(|stack| stack.quantity),
        Some(9)
    );

    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
    );
    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );
    assert_eq!(
        server
            .resource_nodes
            .get(&99)
            .and_then(|node| node.storage.first())
            .map(|stack| stack.quantity),
        Some(3)
    );

    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
            seq: 0,
        }),
    );
    assert_eq!(
        server
            .resource_nodes
            .get(&99)
            .and_then(|node| node.storage.first())
            .map(|stack| stack.quantity),
        Some(3)
    );
}
