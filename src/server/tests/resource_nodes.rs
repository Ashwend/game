use super::*;

fn coal_node(id: u64, quantity: u16) -> ResourceNodeState {
    ResourceNodeState {
        id,
        definition_id: COAL_NODE_ID.to_owned(),
        position: Vec3Net::new(0.0, 0.0, -2.2),
        yaw: 0.0,
        storage: vec![ItemStack::new(COAL_ID, quantity)],
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
    connect_host(&mut server);

    let snapshot = server.snapshot();
    let inventory = &snapshot.players[0].inventory;

    assert_eq!(
        inventory.actionbar_slots[0]
            .as_ref()
            .map(|stack| stack.item_id.as_str()),
        Some(BASIC_HATCHET_ID)
    );
    assert_eq!(
        inventory.actionbar_slots[1]
            .as_ref()
            .map(|stack| stack.item_id.as_str()),
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
fn pickaxe_gathers_materials_and_deletes_empty_node() {
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

    let snapshot = server.snapshot();
    assert!(snapshot.resource_nodes.is_empty());
    assert!(
        snapshot.players[0]
            .inventory
            .inventory_slots
            .iter()
            .any(|slot| slot
                .as_ref()
                .is_some_and(|stack| stack.item_id == COAL_ID && stack.quantity == 3))
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
