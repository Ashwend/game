use super::*;

fn coal_node(id: u64, quantity: u16) -> ResourceNodeState {
    ResourceNodeState {
        id,
        definition_id: COAL_NODE_ID.to_owned(),
        position: Vec3Net::new(0.0, 0.0, -2.2),
        yaw: 0.0,
        storage: vec![ItemStack::new(COAL_ID, quantity)],
        respawn_progress: None,
    }
}

fn look_at_test_node(server: &mut GameServer, client_id: ClientId) {
    let mut movement = movement(1, Vec3Net::ZERO);
    movement.pitch = -0.42;
    server.receive(client_id, ClientMessage::Movement(movement));
}

#[test]
fn connect_seeds_hatchet_and_pickaxe_on_actionbar() {
    let mut server = server();
    let client_id = connect_host(&mut server);

    let snapshot = server.snapshot_for(client_id);
    let inventory = snapshot.players[0]
        .inventory
        .as_ref()
        .expect("host inventory should be present");

    assert_eq!(
        inventory.actionbar_slots[0]
            .as_ref()
            .map(|stack| stack.item_id.as_ref()),
        Some(BASIC_HATCHET_ID)
    );
    assert_eq!(
        inventory.actionbar_slots[1]
            .as_ref()
            .map(|stack| stack.item_id.as_ref()),
        Some(BASIC_PICKAXE_ID)
    );
}

#[test]
fn test_world_spawns_authoritative_resource_nodes() {
    let mut server = server();
    connect_host(&mut server);

    let snapshot = server.snapshot();

    assert!(snapshot.resource_nodes.len() >= 6);
    assert!(
        snapshot
            .resource_nodes
            .iter()
            .any(|node| node.definition_id == COAL_NODE_ID)
    );
}

#[test]
fn pickaxe_gathers_materials_and_starts_node_respawn() {
    let mut server = server();
    let client_id = connect_host(&mut server);
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
        }),
    );

    let snapshot = server.snapshot_for(client_id);
    let inventory = snapshot.players[0]
        .inventory
        .as_ref()
        .expect("host inventory should be present");
    // The node is now a ghost waiting to regrow rather than being deleted.
    // It still appears in the snapshot so the client can render the ghost
    // and the respawn timer can advance.
    assert_eq!(snapshot.resource_nodes.len(), 1);
    let remaining = &snapshot.resource_nodes[0];
    assert_eq!(remaining.id, 99);
    assert!(remaining.storage.iter().all(|stack| stack.quantity == 0));
    assert_eq!(remaining.respawn_progress, Some(0.0));
    assert!(inventory.inventory_slots.iter().any(|slot| {
        slot.as_ref()
            .is_some_and(|stack| stack.item_id.as_ref() == COAL_ID && stack.quantity == 3)
    }));
}

#[test]
fn regenerating_node_rejects_further_gathers_and_then_restocks() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 1));
    look_at_test_node(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
    );

    // First gather depletes the storage and flips the node into the
    // regenerating state.
    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
        }),
    );
    let after_first = server.snapshot();
    assert_eq!(after_first.resource_nodes[0].respawn_progress, Some(0.0));

    // A second gather attempt against the same ghost should produce no
    // toasts, no impacts, no inventory change.
    let inventory_before = {
        let client = server.clients.get(&client_id).expect("host client");
        client.inventory.clone()
    };
    let envelopes = server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
        }),
    );
    assert!(
        envelopes.is_empty(),
        "regenerating node must not respond to gather"
    );
    {
        let client = server.clients.get(&client_id).expect("host client");
        assert_eq!(client.inventory, inventory_before);
    }

    // Driving the tick fast enough times eventually completes the regrow.
    // RESPAWN_DURATION_SECONDS is private; use a clearly-large dt budget.
    for _ in 0..2000 {
        server.tick(0.05);
        if server.snapshot().resource_nodes[0]
            .respawn_progress
            .is_none()
        {
            break;
        }
    }
    let restocked = &server.snapshot().resource_nodes[0];
    assert!(
        restocked.respawn_progress.is_none(),
        "respawn should finish"
    );
    assert!(
        restocked.storage.iter().any(|stack| stack.quantity > 0),
        "storage should refill from the definition once the node is ready"
    );
}

#[test]
fn successful_gather_emits_success_toast_to_requesting_client() {
    use crate::protocol::{ServerMessage, ToastKind};

    let mut server = server();
    let client_id = connect_host(&mut server);
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
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 5));
    look_at_test_node(&mut server, client_id);
    server.receive(
        client_id,
        ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
    );

    // Saturate every inventory slot with a non-stackable item so the coal
    // payout has nowhere to land.
    let client = server
        .clients
        .get_mut(&client_id)
        .expect("connected host should exist");
    for slot in client.inventory.inventory_slots.iter_mut() {
        *slot = Some(ItemStack::new(TEST_RELIC_ID, 1));
    }
    for (index, slot) in client.inventory.actionbar_slots.iter_mut().enumerate() {
        if index == 1 {
            // Keep the pickaxe equipped on slot 1.
            continue;
        }
        *slot = Some(ItemStack::new(TEST_RELIC_ID, 1));
    }
    let tick_before = server.tick;

    let envelopes = server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
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
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 5));
    look_at_test_node(&mut server, client_id);
    // Holding the hatchet (slot 0) instead of the pickaxe means the tool does
    // not allow harvesting the coal node; no toast should fire.

    let envelopes = server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
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
        super::DeliveryTarget::BroadcastExcept(client_id),
        "the swinger's client already played the impact locally; the echo \
         must skip them",
    );
    assert_eq!(kind, ResourceImpactKind::OreNode);
    assert_eq!(position, Vec3Net::new(0.0, 0.0, -2.2));
}

#[test]
fn failed_gather_emits_no_impact_broadcast() {
    use crate::protocol::ServerMessage;

    let mut server = server();
    let client_id = connect_host(&mut server);
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 5));
    look_at_test_node(&mut server, client_id);
    // Still holding the hatchet at slot 0 — wrong tool for coal.

    let envelopes = server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
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
    server.resource_nodes.clear();
    server.resource_nodes.insert(99, coal_node(99, 9));
    look_at_test_node(&mut server, client_id);

    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
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
        }),
    );
    assert_eq!(
        server
            .resource_nodes
            .get(&99)
            .and_then(|node| node.storage.first())
            .map(|stack| stack.quantity),
        Some(6)
    );

    server.receive(
        client_id,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: 99,
        }),
    );
    assert_eq!(
        server
            .resource_nodes
            .get(&99)
            .and_then(|node| node.storage.first())
            .map(|stack| stack.quantity),
        Some(6)
    );
}
