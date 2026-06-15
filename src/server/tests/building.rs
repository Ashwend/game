//! Server-authority tests for the base-building system: building plan
//! placement (snapping, costs), hammer repair/upgrade/demolish, and the
//! raid-balance damage rules.

use super::*;
use crate::{
    building::{BuildingPiece, BuildingTier, FOUNDATION_HEIGHT_M},
    game_balance::{
        BUILDING_DEMOLISH_WINDOW_TICKS, BUILDING_HEWN_WOOD_COST_WALL,
        BUILDING_STICKS_COST_FOUNDATION, BUILDING_STICKS_COST_WALL,
    },
    items::{DeployableKind, HAMMER_ID, IRON_PICKAXE_ID, WOOD_ID},
    protocol::{BuildingCommand, DamageDeployableCommand, DeployedEntityId, PlaceBuildingCommand},
};

fn connect_other(server: &mut GameServer, account_id: u64, name: &str) -> ClientId {
    let client_id = server
        .connect(
            crate::protocol::PROTOCOL_VERSION,
            Some(crate::protocol::GAME_VERSION.to_owned()),
            account_id,
            name.to_owned(),
            String::new(),
        )
        .expect("connect should succeed")
        .0;
    server
        .clients
        .get_mut(&client_id)
        .expect("connected client")
        .controller
        .position = Vec3Net::ZERO;
    client_id
}

fn give(server: &mut GameServer, client_id: ClientId, item_id: &str, quantity: u16) {
    let client = server.clients.get_mut(&client_id).expect("client");
    for slot in client.inventory.inventory_slots.iter_mut() {
        if slot.is_none() {
            *slot = Some(ItemStack::new(item_id, quantity));
            return;
        }
    }
    panic!("no free inventory slot");
}

fn equip(server: &mut GameServer, client_id: ClientId, item_id: &str) {
    let client = server.clients.get_mut(&client_id).expect("client");
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(item_id, 1));
    client.inventory.active_actionbar_slot = 0;
}

fn place(
    server: &mut GameServer,
    client_id: ClientId,
    piece: BuildingPiece,
    position: Vec3Net,
    yaw: f32,
) {
    server.receive(
        client_id,
        ClientMessage::PlaceBuilding(PlaceBuildingCommand {
            piece,
            position,
            yaw,
        }),
    );
}

fn building_ids(server: &GameServer, piece: BuildingPiece) -> Vec<DeployedEntityId> {
    let mut ids: Vec<_> = server
        .deployed_entities
        .values()
        .filter(
            |entity| matches!(entity.kind, DeployableKind::Building { piece: p, .. } if p == piece),
        )
        .map(|entity| entity.id)
        .collect();
    ids.sort_unstable();
    ids
}

#[test]
fn foundation_placement_consumes_sticks_and_spawns_at_sticks_tier() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(2.0, 0.0, 0.0),
        0.3, // off-grid yaw, must snap to a quarter turn
    );

    let ids = building_ids(&server, BuildingPiece::Foundation);
    assert_eq!(ids.len(), 1, "foundation should place");
    let entity = &server.deployed_entities[&ids[0]];
    assert_eq!(
        entity.kind,
        DeployableKind::Building {
            piece: BuildingPiece::Foundation,
            tier: BuildingTier::Sticks,
        }
    );
    assert_eq!(entity.yaw, 0.0, "yaw snaps to the quarter-turn grid");
    let remaining =
        crate::inventory::count_items_in_inventory(&server.clients[&client_id].inventory, WOOD_ID);
    assert_eq!(remaining, 200 - u32::from(BUILDING_STICKS_COST_FOUNDATION));
}

#[test]
fn placement_success_toast_reports_the_resource_spend() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    let out = server.apply_place_building_command(
        client_id,
        PlaceBuildingCommand {
            piece: BuildingPiece::Foundation,
            position: Vec3Net::new(2.0, 0.0, 0.0),
            yaw: 0.0,
        },
    );

    let material = crate::items::item_definition(WOOD_ID)
        .map(|definition| definition.name)
        .expect("wood is a known item");
    let toast = out
        .iter()
        .find_map(|envelope| match &envelope.message {
            crate::protocol::ServerMessage::Toast(toast) => Some(toast),
            _ => None,
        })
        .expect("placement emits a toast");
    // The feedback is the spend ("-{cost} {material}"), not the piece name; the
    // placed structure is visible, the resource subtraction isn't.
    assert_eq!(
        toast.text,
        format!("-{BUILDING_STICKS_COST_FOUNDATION} {material}")
    );
    assert!(
        !toast.text.contains("Placed"),
        "the placed piece name is no longer announced"
    );
}

#[test]
fn destroying_a_furnace_spills_fuel_and_smelt_slots_as_a_loot_bag() {
    use crate::items::{CRUDE_FURNACE_ID, IRON_ORE_ID, WOOD_ID, intern_item_id};
    use crate::protocol::{ItemStack, PlaceDeployableCommand};

    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, CRUDE_FURNACE_ID, 1);
    server.receive(
        client_id,
        ClientMessage::PlaceDeployable(PlaceDeployableCommand {
            item_id: intern_item_id(CRUDE_FURNACE_ID),
            position: Vec3Net::new(0.0, 0.0, 2.0),
            yaw: 0.0,
            wall_mounted: false,
        }),
    );
    let id = *server
        .deployed_entities
        .keys()
        .next()
        .expect("furnace placed");
    {
        let furnace = server
            .deployed_entities
            .get_mut(&id)
            .unwrap()
            .furnace
            .as_mut()
            .expect("furnace state");
        furnace.fuel = Some(ItemStack::new(WOOD_ID, 30));
        furnace.items[0] = Some(ItemStack::new(IRON_ORE_ID, 10));
    }

    server.destroy_deployed_entity(id);
    let bag = server.loot_bags.values().next().expect("contents spilled");
    let mut spilled: Vec<(String, u16)> = bag
        .slots
        .iter()
        .flatten()
        .map(|stack| (stack.item_id.as_ref().to_owned(), stack.quantity))
        .collect();
    spilled.sort();
    assert_eq!(
        spilled,
        vec![(IRON_ORE_ID.to_owned(), 10), (WOOD_ID.to_owned(), 30),]
    );
}

#[test]
fn melee_damage_reaches_a_wide_piece_whose_centre_is_out_of_range() {
    use crate::items::BASIC_HATCHET_ID;

    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);
    // Foundation centre 4 m out: past the 3 m melee radius, but its
    // near edge is 2.5 m away, well within a real swing. The old
    // centre-distance check silently dropped this hit.
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 4.0),
        0.0,
    );
    let id = building_ids(&server, BuildingPiece::Foundation)[0];
    let before = server.deployed_entities[&id].health;

    equip(&mut server, client_id, BASIC_HATCHET_ID);
    server.receive(
        client_id,
        ClientMessage::DamageDeployable(DamageDeployableCommand { id }),
    );
    assert!(
        server.deployed_entities[&id].health < before,
        "an edge-range swing must land"
    );
}

#[test]
fn hammer_repairs_crafted_deployables_with_the_primary_material() {
    use crate::items::{CRUDE_FURNACE_ID, STONE_ID, intern_item_id};
    use crate::protocol::PlaceDeployableCommand;

    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, CRUDE_FURNACE_ID, 1);
    server.receive(
        client_id,
        ClientMessage::PlaceDeployable(PlaceDeployableCommand {
            item_id: intern_item_id(CRUDE_FURNACE_ID),
            position: Vec3Net::new(0.0, 0.0, 2.0),
            yaw: 0.0,
            wall_mounted: false,
        }),
    );
    let id = *server
        .deployed_entities
        .keys()
        .next()
        .expect("furnace placed");
    let max_health = server.deployed_entities[&id].max_health;
    server.deployed_entities.get_mut(&id).unwrap().health = max_health / 2;

    equip(&mut server, client_id, HAMMER_ID);
    give(&mut server, client_id, STONE_ID, 100);

    // One hit: a quarter of max HP back, a quarter of the recipe's
    // primary input (the furnace crafts from 60 stone, so 15) consumed.
    server.receive(
        client_id,
        ClientMessage::Building(BuildingCommand::Repair { id }),
    );
    let healed = server.deployed_entities[&id].health;
    assert_eq!(healed, max_health / 2 + max_health / 4);
    let stone =
        crate::inventory::count_items_in_inventory(&server.clients[&client_id].inventory, STONE_ID);
    assert_eq!(stone, 85, "repair consumes a quarter of the primary input");

    // Without the material the hit is refused and heals nothing.
    let client = server.clients.get_mut(&client_id).unwrap();
    client.next_gather_tick = 0;
    for slot in client.inventory.inventory_slots.iter_mut() {
        if slot
            .as_ref()
            .is_some_and(|stack| stack.item_id.as_ref() == STONE_ID)
        {
            *slot = None;
        }
    }
    server.receive(
        client_id,
        ClientMessage::Building(BuildingCommand::Repair { id }),
    );
    assert_eq!(server.deployed_entities[&id].health, healed);
}

#[test]
fn free_deployables_stand_on_platforms_and_fall_with_them() {
    use crate::items::{CRUDE_FURNACE_ID, STORAGE_BOX_SMALL_ID, intern_item_id};
    use crate::protocol::{ItemStack, PlaceDeployableCommand};

    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );

    // A furnace placed on the foundation's walkable top sticks.
    give(&mut server, client_id, CRUDE_FURNACE_ID, 1);
    server.receive(
        client_id,
        ClientMessage::PlaceDeployable(PlaceDeployableCommand {
            item_id: intern_item_id(CRUDE_FURNACE_ID),
            position: Vec3Net::new(0.0, 0.5, 2.0),
            yaw: 0.0,
            wall_mounted: false,
        }),
    );
    let furnaces = |server: &GameServer| {
        server
            .deployed_entities
            .values()
            .filter(|entity| matches!(entity.kind, DeployableKind::Furnace { .. }))
            .count()
    };
    assert_eq!(furnaces(&server), 1, "furnace stands on the platform top");

    // Mid-air (no surface at that height): rejected.
    give(&mut server, client_id, CRUDE_FURNACE_ID, 1);
    server.receive(
        client_id,
        ClientMessage::PlaceDeployable(PlaceDeployableCommand {
            item_id: intern_item_id(CRUDE_FURNACE_ID),
            position: Vec3Net::new(0.0, 1.4, 2.0),
            yaw: 0.0,
            wall_mounted: false,
        }),
    );
    assert_eq!(furnaces(&server), 1, "mid-air placement must be refused");

    // A loaded storage box on the same platform spills when the
    // platform is destroyed, and the furnace falls with it.
    give(&mut server, client_id, STORAGE_BOX_SMALL_ID, 1);
    server.receive(
        client_id,
        ClientMessage::PlaceDeployable(PlaceDeployableCommand {
            item_id: intern_item_id(STORAGE_BOX_SMALL_ID),
            position: Vec3Net::new(1.0, 0.5, 3.0),
            yaw: 0.0,
            wall_mounted: false,
        }),
    );
    let box_id = server
        .deployed_entities
        .values()
        .find(|entity| matches!(entity.kind, DeployableKind::StorageBox { .. }))
        .map(|entity| entity.id)
        .expect("box on the platform");
    server
        .deployed_entities
        .get_mut(&box_id)
        .unwrap()
        .storage
        .as_mut()
        .unwrap()
        .slots[0] = Some(ItemStack::new(crate::items::WOOD_ID, 12));

    let foundation = building_ids(&server, BuildingPiece::Foundation)[0];
    server.destroy_deployed_entity(foundation);
    assert_eq!(furnaces(&server), 0, "the furnace fell with its floor");
    assert!(
        !server.deployed_entities.contains_key(&box_id),
        "the box fell with its floor"
    );
    let bag = server.loot_bags.values().next().expect("contents spilled");
    assert_eq!(bag.slots[0].as_ref().unwrap().quantity, 12);
}

#[test]
fn raised_foundations_place_inside_the_band_and_extensions_inherit_height() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    // Above the raise band: refused.
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(2.0, crate::game_balance::FOUNDATION_RAISE_MAX_M + 0.5, 0.0),
        0.0,
    );
    assert!(building_ids(&server, BuildingPiece::Foundation).is_empty());

    // Inside the band: a stilted slab.
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(2.0, 1.2, 0.0),
        0.0,
    );
    let ids = building_ids(&server, BuildingPiece::Foundation);
    assert_eq!(ids.len(), 1, "raised foundation should place");
    assert!((server.deployed_entities[&ids[0]].position.y - 1.2).abs() < 1e-4);

    // A snapped extension keeps the neighbour's height even when the
    // request arrives at ground level.
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(5.0, 0.0, 0.0),
        0.0,
    );
    let ids = building_ids(&server, BuildingPiece::Foundation);
    assert_eq!(ids.len(), 2, "extension should place");
    let extension = &server.deployed_entities[&ids[1]];
    assert!(
        (extension.position.y - 1.2).abs() < 1e-4,
        "extension inherits the founder's height, got {}",
        extension.position.y
    );

    // A foundation overlapping the raised slab's footprint at a
    // different height hits the ground-reaching skirt collider.
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(3.0, 0.0, 1.0),
        0.0,
    );
    assert_eq!(
        building_ids(&server, BuildingPiece::Foundation).len(),
        2,
        "offset-height overlap must be refused"
    );
}

#[test]
fn foundation_placement_fails_without_materials_and_consumes_nothing() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 5);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(2.0, 0.0, 0.0),
        0.0,
    );

    assert!(building_ids(&server, BuildingPiece::Foundation).is_empty());
    let remaining =
        crate::inventory::count_items_in_inventory(&server.clients[&client_id].inventory, WOOD_ID);
    assert_eq!(remaining, 5, "a failed placement must not eat materials");
}

#[test]
fn walls_snap_to_foundation_sockets_and_reject_free_placement() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    // No foundation yet: a wall request goes nowhere.
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(2.0, 0.0, 0.0),
        0.0,
    );
    assert!(building_ids(&server, BuildingPiece::Wall).is_empty());

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    // Aim near the foundation's +Z edge socket (at z = 2.0 + 1.5).
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.2, 0.0, 3.4),
        0.0,
    );
    let walls = building_ids(&server, BuildingPiece::Wall);
    assert_eq!(walls.len(), 1, "wall should snap onto the edge socket");
    let wall = &server.deployed_entities[&walls[0]];
    assert!((wall.position.x - 0.0).abs() < 1e-4);
    assert!((wall.position.z - 3.5).abs() < 1e-4);
    assert!(
        (wall.position.y - FOUNDATION_HEIGHT_M).abs() < 1e-4,
        "walls sit on the foundation top"
    );

    // The same socket can't host a second wall-like piece.
    place(
        &mut server,
        client_id,
        BuildingPiece::Doorway,
        Vec3Net::new(0.0, 0.0, 3.4),
        0.0,
    );
    assert!(building_ids(&server, BuildingPiece::Doorway).is_empty());
}

#[test]
fn adjacent_foundations_snap_onto_the_grid() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 1.0),
        0.0,
    );
    // Aim roughly one cell toward +X; should land exactly 3 m over.
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(3.3, 0.0, 0.8),
        0.0,
    );
    let ids = building_ids(&server, BuildingPiece::Foundation);
    assert_eq!(ids.len(), 2);
    let second = &server.deployed_entities[&ids[1]];
    assert!((second.position.x - 3.0).abs() < 1e-4);
    assert!((second.position.z - 1.0).abs() < 1e-4);
}

#[test]
fn upgrade_walks_tiers_refills_health_and_requires_owner() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);
    give(&mut server, client_id, crate::items::HEWN_LOG_ID, 200);
    equip(&mut server, client_id, HAMMER_ID);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 0.0, 3.5),
        0.0,
    );
    let wall_id = building_ids(&server, BuildingPiece::Wall)[0];

    // Chip the wall so the upgrade's full heal is observable.
    if let Some(entity) = server.deployed_entity_mut(wall_id) {
        entity.health = 10;
    }

    server.receive(
        client_id,
        ClientMessage::Building(BuildingCommand::Upgrade { id: wall_id }),
    );
    let wall = &server.deployed_entities[&wall_id];
    assert_eq!(
        wall.kind,
        DeployableKind::Building {
            piece: BuildingPiece::Wall,
            tier: BuildingTier::HewnWood,
        }
    );
    assert_eq!(wall.health, wall.max_health, "upgrade refills health");
    let hewn_logs_left = crate::inventory::count_items_in_inventory(
        &server.clients[&client_id].inventory,
        crate::items::HEWN_LOG_ID,
    );
    assert_eq!(
        hewn_logs_left,
        200 - u32::from(BUILDING_HEWN_WOOD_COST_WALL)
    );

    // A different player can't upgrade someone else's wall.
    let intruder = connect_other(&mut server, 2, "Intruder");
    give(&mut server, intruder, crate::items::STONE_ID, 200);
    equip(&mut server, intruder, HAMMER_ID);
    server.receive(
        intruder,
        ClientMessage::Building(BuildingCommand::Upgrade { id: wall_id }),
    );
    let wall = &server.deployed_entities[&wall_id];
    assert!(
        matches!(
            wall.kind,
            DeployableKind::Building {
                tier: BuildingTier::HewnWood,
                ..
            }
        ),
        "non-owner upgrade must be rejected"
    );
}

#[test]
fn repair_restores_health_and_consumes_tier_materials() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);
    equip(&mut server, client_id, HAMMER_ID);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    let id = building_ids(&server, BuildingPiece::Foundation)[0];
    if let Some(entity) = server.deployed_entity_mut(id) {
        entity.health = 1;
    }
    let wood_before =
        crate::inventory::count_items_in_inventory(&server.clients[&client_id].inventory, WOOD_ID);

    server.receive(
        client_id,
        ClientMessage::Building(BuildingCommand::Repair { id }),
    );

    let entity = &server.deployed_entities[&id];
    assert!(entity.health > 1, "repair should restore health");
    let wood_after =
        crate::inventory::count_items_in_inventory(&server.clients[&client_id].inventory, WOOD_ID);
    assert!(wood_after < wood_before, "repair costs materials");
}

#[test]
fn demolish_window_closes_after_fifteen_minutes() {
    let mut server = server();
    // A regular (non-admin) builder: the host is an admin and admins
    // bypass the demolish window for moderation.
    let client_id = connect_other(&mut server, 2, "Builder");
    give(&mut server, client_id, WOOD_ID, 200);
    equip(&mut server, client_id, HAMMER_ID);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    let id = building_ids(&server, BuildingPiece::Foundation)[0];

    // Push time past the window; demolish must be refused.
    server.tick += BUILDING_DEMOLISH_WINDOW_TICKS + 1;
    server.receive(
        client_id,
        ClientMessage::Building(BuildingCommand::Demolish { id }),
    );
    assert!(
        server.deployed_entities.contains_key(&id),
        "set structures can't be hammer-demolished"
    );

    // A fresh placement inside the window demolishes fine.
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, -2.0),
        0.0,
    );
    let fresh = *building_ids(&server, BuildingPiece::Foundation)
        .iter()
        .find(|fresh_id| **fresh_id != id)
        .expect("second foundation");
    server.receive(
        client_id,
        ClientMessage::Building(BuildingCommand::Demolish { id: fresh }),
    );
    assert!(!server.deployed_entities.contains_key(&fresh));
}

#[test]
fn raid_balance_anyone_damages_buildings_but_stone_is_tool_immune() {
    let mut server = server();
    let owner = connect_host(&mut server);
    give(&mut server, owner, WOOD_ID, 200);
    place(
        &mut server,
        owner,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    let id = building_ids(&server, BuildingPiece::Foundation)[0];

    // A different player with an iron pickaxe chews through sticks.
    let raider = connect_other(&mut server, 2, "Raider");
    equip(&mut server, raider, IRON_PICKAXE_ID);
    let before = server.deployed_entities[&id].health;
    server.receive(
        raider,
        ClientMessage::DamageDeployable(DamageDeployableCommand { id }),
    );
    let after = server.deployed_entities[&id].health;
    assert!(
        after < before,
        "non-owners must be able to damage buildings (raiding)"
    );

    // Flip the same piece to stone tier: tools deal exactly zero.
    if let Some(entity) = server.deployed_entity_mut(id) {
        entity.kind = DeployableKind::Building {
            piece: BuildingPiece::Foundation,
            tier: BuildingTier::Stone,
        };
        entity.max_health = 9_000;
        entity.health = 9_000;
    }
    // Clear the swing cooldown applied by the first hit.
    server.clients.get_mut(&raider).unwrap().next_gather_tick = 0;
    server.receive(
        raider,
        ClientMessage::DamageDeployable(DamageDeployableCommand { id }),
    );
    assert_eq!(
        server.deployed_entities[&id].health, 9_000,
        "stone-tier buildings are immune to tools"
    );
}

#[test]
fn ceilings_need_wall_support_and_stack_storeys() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    // No walls on the storey: the roof has nothing to rest on.
    place(
        &mut server,
        client_id,
        BuildingPiece::Ceiling,
        Vec3Net::new(0.0, 3.5, 2.0),
        0.0,
    );
    assert!(building_ids(&server, BuildingPiece::Ceiling).is_empty());

    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 0.0, 3.4),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Ceiling,
        Vec3Net::new(0.0, 3.3, 2.0),
        0.0,
    );
    let ceilings = building_ids(&server, BuildingPiece::Ceiling);
    assert_eq!(ceilings.len(), 1, "a walled storey takes a ceiling");
    let ceiling = &server.deployed_entities[&ceilings[0]];
    assert!((ceiling.position.x - 0.0).abs() < 1e-4);
    assert!(
        (ceiling.position.y - 3.3).abs() < 1e-4,
        "ceiling nests into the wall band, top flush with the wall tops"
    );
    assert!((ceiling.position.z - 2.0).abs() < 1e-4);

    // The cell is roofed; a duplicate is rejected by the box overlap.
    place(
        &mut server,
        client_id,
        BuildingPiece::Ceiling,
        Vec3Net::new(0.0, 3.3, 2.0),
        0.0,
    );
    assert_eq!(building_ids(&server, BuildingPiece::Ceiling).len(), 1);

    // Second-storey walls mount on the ceiling's edge sockets, at the
    // exact same height a wall stacked on a wall would take.
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 3.5, 3.5),
        0.0,
    );
    let walls = building_ids(&server, BuildingPiece::Wall);
    assert_eq!(walls.len(), 2, "the ceiling edge takes a storey-two wall");
    let upper = server
        .deployed_entities
        .values()
        .find(|entity| {
            matches!(
                entity.kind,
                DeployableKind::Building {
                    piece: BuildingPiece::Wall,
                    ..
                }
            ) && entity.position.y > 1.0
        })
        .expect("upper wall");
    assert!(
        (upper.position.y - 3.5).abs() < 1e-4,
        "storey-two walls start exactly one wall height up"
    );
}

#[test]
fn stairs_need_a_platform_and_a_clear_cell() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    // No platform under the aim: rejected.
    place(
        &mut server,
        client_id,
        BuildingPiece::Stairs,
        Vec3Net::new(0.0, 0.5, 2.0),
        0.0,
    );
    assert!(building_ids(&server, BuildingPiece::Stairs).is_empty());

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Stairs,
        Vec3Net::new(0.0, 0.5, 2.0),
        0.0,
    );
    let stairs = building_ids(&server, BuildingPiece::Stairs);
    assert_eq!(stairs.len(), 1, "stairs stand on the foundation cell");
    let flight = &server.deployed_entities[&stairs[0]];
    assert!(
        (flight.position.y - FOUNDATION_HEIGHT_M).abs() < 1e-4,
        "stairs base sits on the platform top"
    );

    // Same cell again: the flights' boxes overlap, rejected.
    place(
        &mut server,
        client_id,
        BuildingPiece::Stairs,
        Vec3Net::new(0.0, 0.5, 2.0),
        0.0,
    );
    assert_eq!(building_ids(&server, BuildingPiece::Stairs).len(), 1);

    // A ceiling can't roof the stairs cell: the flight rises through it.
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 0.0, 3.4),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Ceiling,
        Vec3Net::new(0.0, 3.3, 2.0),
        0.0,
    );
    assert!(
        building_ids(&server, BuildingPiece::Ceiling).is_empty(),
        "the flight needs the cell above open"
    );
}

#[test]
fn destroying_a_foundation_collapses_the_structure_above() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);
    give(&mut server, client_id, WOOD_ID, 200);
    give(&mut server, client_id, crate::items::HEWN_LOG_DOOR_ID, 1);

    // Foundation A: wall (+Z), doorway (-Z) carrying a door, ceiling,
    // and a second-storey wall on the ceiling edge.
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 0.5, 3.5),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Doorway,
        Vec3Net::new(0.0, 0.5, 0.5),
        0.0,
    );
    let doorway_id = building_ids(&server, BuildingPiece::Doorway)[0];
    server.receive(
        client_id,
        ClientMessage::Door(crate::protocol::DoorCommand::Place {
            doorway_id,
            flip: false,
            code: "1234".to_owned(),
        }),
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Ceiling,
        Vec3Net::new(0.0, 3.3, 2.0),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 3.5, 3.5),
        0.0,
    );

    // Foundation B next door with stairs: unrelated, must survive.
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(3.0, 0.0, 2.0),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Stairs,
        Vec3Net::new(3.0, 0.5, 2.0),
        0.0,
    );

    let doors = |server: &GameServer| {
        server
            .deployed_entities
            .values()
            .filter(|entity| matches!(entity.kind, DeployableKind::Door))
            .count()
    };
    assert_eq!(building_ids(&server, BuildingPiece::Wall).len(), 2);
    assert_eq!(building_ids(&server, BuildingPiece::Ceiling).len(), 1);
    assert_eq!(doors(&server), 1);

    let foundation_a = *building_ids(&server, BuildingPiece::Foundation)
        .iter()
        .find(|id| server.deployed_entities[id].position.x.abs() < 0.1)
        .expect("foundation A");
    server.destroy_deployed_entity(foundation_a);

    // Everything the foundation held up is gone, transitively: walls,
    // doorway, the mounted door, the ceiling, and the storey-two wall.
    assert!(building_ids(&server, BuildingPiece::Wall).is_empty());
    assert!(building_ids(&server, BuildingPiece::Doorway).is_empty());
    assert!(building_ids(&server, BuildingPiece::Ceiling).is_empty());
    assert_eq!(doors(&server), 0);
    // The neighbouring base keeps standing.
    assert_eq!(building_ids(&server, BuildingPiece::Foundation).len(), 1);
    assert_eq!(building_ids(&server, BuildingPiece::Stairs).len(), 1);
}

fn move_player(server: &mut GameServer, client_id: ClientId, x: f32, z: f32) {
    server
        .clients
        .get_mut(&client_id)
        .expect("client")
        .controller
        .position = Vec3Net::new(x, 0.0, z);
}

#[test]
fn walls_stack_on_walls_and_stability_decays_per_storey() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 0.5, 3.5),
        0.0,
    );
    // Stack a second wall directly on the first: no ceiling required.
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 3.5, 3.5),
        0.0,
    );
    let walls = building_ids(&server, BuildingPiece::Wall);
    assert_eq!(walls.len(), 2, "walls stack on walls");
    let lower = &server.deployed_entities[&walls[0]];
    let upper = &server.deployed_entities[&walls[1]];
    assert_eq!(lower.stability, 90, "wall on a foundation keeps 90%");
    assert_eq!(upper.stability, 81, "each storey keeps 90% of the last");

    // A duplicate in the same span (the near-coincident ceiling-edge /
    // wall-top slots) is refused.
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 3.5, 3.5),
        0.0,
    );
    assert_eq!(building_ids(&server, BuildingPiece::Wall).len(), 2);
}

#[test]
fn ceiling_ledges_decay_and_reject_past_the_minimum() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);
    give(&mut server, client_id, WOOD_ID, 200);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 0.5, 3.5),
        0.0,
    );
    // The wall carries the cell on its far side too: a ledge with no
    // foundation below it.
    place(
        &mut server,
        client_id,
        BuildingPiece::Ceiling,
        Vec3Net::new(0.0, 3.3, 5.0),
        0.0,
    );
    let first = building_ids(&server, BuildingPiece::Ceiling);
    assert_eq!(first.len(), 1, "walls carry ceilings on either side");
    assert_eq!(
        server.deployed_entities[&first[0]].stability, 81,
        "carried ceiling keeps 90% of the wall's 90%"
    );

    // Cantilever outward at 35% per tile: 81 -> 28 places, the next
    // tile would compute 9%, under the 10% placement minimum, and is
    // refused. Roofs come from walls, not chains of ledges.
    move_player(&mut server, client_id, 0.0, 6.0);
    place(
        &mut server,
        client_id,
        BuildingPiece::Ceiling,
        Vec3Net::new(0.0, 3.3, 8.0),
        0.0,
    );
    let ceilings = building_ids(&server, BuildingPiece::Ceiling);
    assert_eq!(ceilings.len(), 2, "the first cantilever tile places");
    let newest = *ceilings.last().expect("ceiling placed");
    assert_eq!(
        server.deployed_entities[&newest].stability, 28,
        "cantilever tile keeps 35% of its neighbour's stability"
    );
    move_player(&mut server, client_id, 0.0, 9.0);
    let out = server.apply_place_building_command(
        client_id,
        PlaceBuildingCommand {
            piece: BuildingPiece::Ceiling,
            position: Vec3Net::new(0.0, 3.3, 11.0),
            yaw: 0.0,
        },
    );
    assert!(
        out.iter().any(|envelope| matches!(
            &envelope.message,
            crate::protocol::ServerMessage::Toast(toast)
                if toast.text.contains("support")
        )),
        "the second cantilever tile must be refused for lack of support"
    );
    assert_eq!(building_ids(&server, BuildingPiece::Ceiling).len(), 2);
}

#[test]
fn losing_the_supporting_wall_drops_the_ledge() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 0.5, 3.5),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Ceiling,
        Vec3Net::new(0.0, 3.3, 5.0),
        0.0,
    );
    let wall = building_ids(&server, BuildingPiece::Wall)[0];
    server.destroy_deployed_entity(wall);
    assert!(
        building_ids(&server, BuildingPiece::Ceiling).is_empty(),
        "a ledge with its only wall gone has zero stability and falls"
    );
    assert_eq!(
        building_ids(&server, BuildingPiece::Foundation).len(),
        1,
        "the foundation stands"
    );
}

#[test]
fn stored_stability_matches_the_placement_prediction() {
    // The placement gate predicts via `candidate_stability_pct`
    // (backward relations); the refresh recomputes via the forward
    // walk. The two must agree or the ghost lies.
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 2.0),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Wall,
        Vec3Net::new(0.0, 0.5, 3.5),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Ceiling,
        Vec3Net::new(0.0, 3.5, 5.0),
        0.0,
    );
    for entity in server.deployed_entities.values() {
        let crate::items::DeployableKind::Building { piece, .. } = entity.kind else {
            continue;
        };
        let predicted = server.building_candidate_stability(piece, entity.position, entity.yaw);
        assert_eq!(
            u32::from(entity.stability),
            predicted,
            "stored vs predicted diverged for {piece:?}"
        );
    }
}

#[test]
fn placement_costs_match_the_balance_table() {
    // Guard against the cost table silently diverging from the wall/
    // foundation split (foundations cost more).
    let foundation = crate::building::placement_cost(BuildingPiece::Foundation);
    let wall = crate::building::placement_cost(BuildingPiece::Wall);
    assert_eq!(foundation.1, BUILDING_STICKS_COST_FOUNDATION);
    assert_eq!(wall.1, BUILDING_STICKS_COST_WALL);
    assert!(foundation.1 > wall.1);
}

#[test]
fn extending_one_grid_cannot_overlap_an_offset_foundation() {
    let mut server = server();
    let client_id = connect_host(&mut server);
    give(&mut server, client_id, WOOD_ID, 200);

    // Free foundation A near the player, free foundation B on an
    // offset grid (its requested spot is outside A's snap tolerance so
    // it stays off-grid).
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 1.0),
        0.0,
    );
    place(
        &mut server,
        client_id,
        BuildingPiece::Foundation,
        Vec3Net::new(0.0, 0.0, 4.8),
        0.0,
    );
    assert_eq!(building_ids(&server, BuildingPiece::Foundation).len(), 2);

    // Extend A toward B: the neighbour cell at z = 4.0 overlaps B
    // (4.0 +- 1.5 vs 4.8 +- 1.5). Must be rejected.
    let out = server.apply_place_building_command(
        client_id,
        PlaceBuildingCommand {
            piece: BuildingPiece::Foundation,
            position: Vec3Net::new(0.0, 0.0, 4.0),
            yaw: 0.0,
        },
    );
    assert!(
        out.iter().any(|envelope| matches!(
            &envelope.message,
            crate::protocol::ServerMessage::Toast(toast)
                if toast.text.contains("in the way")
        )),
        "the overlap rejection should toast"
    );
    assert_eq!(
        building_ids(&server, BuildingPiece::Foundation).len(),
        2,
        "an extension overlapping another foundation must be rejected"
    );
}
