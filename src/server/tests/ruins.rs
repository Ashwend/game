//! Server-side ruin-cache tests: fresh-world spawning, indestructibility, the
//! non-placeable gate, and the refill loop end to end on a real `GameServer`.

use crate::{
    items::{ANCIENT_FITTINGS_ID, DeployableKind, RUIN_CACHE_ID, intern_item_id},
    protocol::{
        ClientMessage, ContainerViewKind, DamageDeployableCommand, LootBagCommand, LootBagSlotRef,
        PlaceDeployableCommand, Vec3Net,
    },
    server::loot_bag::OpenContainer,
    world::{RuinFootprint, ruin_layout},
};

use super::super::test_support::{connect_named, server_with_ruins};

/// Count the ruin caches currently in the server's deployable map.
fn cache_count(server: &crate::server::GameServer) -> usize {
    server
        .deployed_entities
        .values()
        .filter(|e| matches!(e.kind, DeployableKind::RuinCache))
        .count()
}

#[test]
fn fresh_world_spawns_one_cache_per_prefab_cache_point() {
    let server = server_with_ruins();
    let seed = server.save.map.world_seed();
    let dims = server.save.map.chunk_dims();
    // Expected cache count = sum of each site's prefab cache-point count.
    let expected: usize = ruin_layout(seed, dims)
        .iter()
        .map(|site| site.prefab.cache_points().len())
        .sum();
    assert_eq!(
        cache_count(&server),
        expected,
        "a fresh world should spawn exactly one cache per prefab cache point"
    );
    assert!(
        expected > 0,
        "the test world should have at least one ruin cache"
    );
}

#[test]
fn spawned_caches_are_owner_less_and_stocked() {
    let server = server_with_ruins();
    for (map_id, entity) in server.deployed_entities.iter() {
        if !matches!(entity.kind, DeployableKind::RuinCache) {
            continue;
        }
        // The entity's own id must match its map key: the mirror sync builds
        // replication views from `entity.id`, so a stale 0 id (the
        // `DeployedEntity::new` placement default) would collapse every cache
        // onto one bogus replicated entity. Regression guard for exactly that.
        assert_eq!(
            entity.id, *map_id,
            "cache entity id must match its map key or replication collapses"
        );
        assert!(entity.id != 0, "a spawned cache must carry a real id");
        // And the cache must sit at the foundation top, proud on the platform.
        assert!(
            (entity.position.y - crate::building::FOUNDATION_HEIGHT_M).abs() < 1e-3,
            "cache at y={} should spawn at the foundation top",
            entity.position.y
        );
        assert!(entity.owner.is_none(), "a ruin cache has no owner");
        let storage = entity
            .storage
            .as_ref()
            .expect("a cache stores its loot in the storage grid");
        assert!(
            storage.slots.iter().any(Option::is_some),
            "a freshly spawned cache should already hold loot"
        );
        assert!(
            entity.ruin_cache.is_some(),
            "a cache carries its refill bookkeeping"
        );
    }
}

#[test]
fn caches_are_indestructible() {
    let mut server = server_with_ruins();
    let client_id = connect_named(&mut server, "raider");
    // Make the raider an admin so we prove even an admin can't destroy a cache
    // (admins bypass the ownership gate, but the cache reject comes first).
    if let Some(client) = server.clients.get_mut(&client_id) {
        client.is_admin = true;
    }
    let cache_id = *server
        .deployed_entities
        .iter()
        .find(|(_, e)| matches!(e.kind, DeployableKind::RuinCache))
        .map(|(id, _)| id)
        .expect("a cache exists");
    let before = cache_count(&server);
    // Hammer it many times; a cache must never lose health or be removed.
    for _ in 0..50 {
        server.apply_damage_deployable_command(client_id, DamageDeployableCommand { id: cache_id });
    }
    assert_eq!(
        cache_count(&server),
        before,
        "a cache must survive any amount of damage"
    );
    let cache = server
        .deployed_entities
        .get(&cache_id)
        .expect("cache still present");
    assert_eq!(cache.health, cache.max_health, "cache health is untouched");
}

#[test]
fn players_cannot_place_a_cache() {
    let mut server = server_with_ruins();
    let client_id = connect_named(&mut server, "builder");
    // Even if a client forges a place command for the cache item, the server
    // must refuse it (no recipe grants the item, and the place path rejects a
    // non-free-placement / unowned item). We assert no new cache appears.
    let before = cache_count(&server);
    let _ = server.receive(
        client_id,
        ClientMessage::PlaceDeployable(PlaceDeployableCommand {
            item_id: intern_item_id(RUIN_CACHE_ID),
            position: Vec3Net::new(1.0, 0.0, 1.0),
            yaw: 0.0,
            wall_mounted: false,
        }),
    );
    assert_eq!(
        cache_count(&server),
        before,
        "a player place command must never spawn a ruin cache"
    );
}

#[test]
fn emptying_a_cache_schedules_and_fires_a_refill() {
    let mut server = server_with_ruins();
    let cache_id = *server
        .deployed_entities
        .iter()
        .find(|(_, e)| matches!(e.kind, DeployableKind::RuinCache))
        .map(|(id, _)| id)
        .expect("a cache exists");

    // Empty the cache directly (as looting to zero would).
    if let Some(entity) = server.deployed_entities.get_mut(&cache_id)
        && let Some(storage) = entity.storage.as_mut()
    {
        for slot in storage.slots.iter_mut() {
            *slot = None;
        }
    }

    // One tick: the refill should be scheduled but not yet fired.
    server.tick_ruin_caches();
    {
        let cache = server.deployed_entities.get(&cache_id).unwrap();
        let refill = cache.ruin_cache.as_ref().unwrap();
        assert!(
            refill.refill_at_tick.is_some(),
            "emptying a cache schedules a refill"
        );
        assert!(
            cache
                .storage
                .as_ref()
                .unwrap()
                .slots
                .iter()
                .all(Option::is_none),
            "no loot before the timer fires"
        );
    }

    // Fast-forward the server clock past the scheduled fire tick, then tick.
    let fire = server
        .deployed_entities
        .get(&cache_id)
        .unwrap()
        .ruin_cache
        .as_ref()
        .unwrap()
        .refill_at_tick
        .unwrap();
    server.tick = fire;
    server.tick_ruin_caches();

    let cache = server.deployed_entities.get(&cache_id).unwrap();
    assert!(
        cache.ruin_cache.as_ref().unwrap().refill_at_tick.is_none(),
        "the schedule clears after firing"
    );
    assert!(
        cache
            .storage
            .as_ref()
            .unwrap()
            .slots
            .iter()
            .any(Option::is_some),
        "the cache is restocked after the refill fires"
    );
    assert!(
        cache
            .storage
            .as_ref()
            .unwrap()
            .slots
            .iter()
            .flatten()
            .any(|s| s.item_id.as_ref() == crate::items::ANCIENT_FITTINGS_ID),
        "a refilled cache always holds ancient fittings"
    );
}

#[test]
fn no_resource_node_spawns_inside_a_cache_footprint() {
    // The whole point of the node-rejection gate: no resource node in the server
    // should sit inside any ruin footprint on a fresh world.
    let server = server_with_ruins();
    let seed = server.save.map.world_seed();
    let dims = server.save.map.chunk_dims();
    let footprints: Vec<RuinFootprint> = crate::world::ruin_footprints(&ruin_layout(seed, dims));
    for (id, node) in server.resource_nodes_iter() {
        for fp in &footprints {
            assert!(
                !fp.contains(node.position.x, node.position.z),
                "resource node {id} spawned inside a ruin footprint"
            );
        }
    }
}

#[test]
fn anyone_can_open_and_loot_a_cache_through_the_container_path() {
    let mut server = server_with_ruins();
    let client_id = connect_named(&mut server, "looter");
    let (cache_id, cache_pos) = server
        .deployed_entities
        .iter()
        .find(|(_, e)| matches!(e.kind, DeployableKind::RuinCache))
        .map(|(id, e)| (*id, e.position))
        .expect("a cache exists");

    // Stand at the cache (the interact range gate is horizontal).
    if let Some(client) = server.clients.get_mut(&client_id) {
        client.controller.position = cache_pos;
    }
    server
        .chunk_manager
        .update_player_chunk(client_id, cache_pos);

    // Tap E: the cache opens through the shared storage container pointer.
    server.receive(client_id, ClientMessage::OpenStorageBox { id: cache_id });
    assert_eq!(
        server.clients[&client_id].open_container,
        Some(OpenContainer::StorageBox(cache_id)),
        "a ruin cache opens as the shared storage container"
    );
    let view = server
        .open_loot_bag_view_for(client_id)
        .expect("an open cache resolves a container view");
    assert_eq!(view.kind, ContainerViewKind::StorageBox);
    let fittings_slot = view
        .slots
        .iter()
        .position(|slot| {
            slot.as_ref()
                .is_some_and(|stack| stack.item_id.as_ref() == ANCIENT_FITTINGS_ID)
        })
        .expect("a stocked cache view shows its fittings");

    // Take the fittings via the shared container move.
    server.receive(
        client_id,
        ClientMessage::LootBag(LootBagCommand::Move {
            from: LootBagSlotRef::Bag(fittings_slot),
            to: LootBagSlotRef::PlayerInventory(0),
            quantity: None,
        }),
    );
    let looted = server.clients[&client_id].inventory.inventory_slots[0]
        .as_ref()
        .expect("the fittings moved into the player inventory");
    assert_eq!(looted.item_id.as_ref(), ANCIENT_FITTINGS_ID);
    assert!(
        server.deployed_entities[&cache_id]
            .storage
            .as_ref()
            .unwrap()
            .slots[fittings_slot]
            .is_none(),
        "the cache slot emptied"
    );
}
