use super::*;
use crate::world::ChunkDims;

#[test]
fn new_for_world_yields_consistent_node_state() {
    let (manager, nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
    assert_eq!(manager.live_node_count(), nodes.len());
    // Every live node should be tracked in node_chunks.
    for state in &nodes {
        assert!(manager.node_chunks.contains_key(&state.id));
    }
}

#[test]
fn save_round_trips_state() {
    let (mut manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
    manager.regrow_queue.push(RegrowEvent {
        fire_tick: 10_000,
        coord: ChunkCoord::new(0, 0),
        kind: NodeKind::TreeMedium,
    });
    let save = manager.to_save(5_000);
    let restored = ChunkManager::from_save(save, 0);
    assert_eq!(restored.live_node_count(), manager.live_node_count());
    assert_eq!(restored.pending_regrow_count(), 1);
}

#[test]
fn nodes_visible_to_returns_within_radius_only() {
    let (manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
    let near = manager.nodes_visible_to(Vec3Net::new(0.0, 0.0, 0.0), ViewRadiusTier::Low);
    let far = manager.nodes_visible_to(Vec3Net::new(0.0, 0.0, 0.0), ViewRadiusTier::High);
    // High view should never see fewer than low view.
    assert!(far.len() >= near.len());
    // Both should be within the total live count.
    assert!(far.len() <= manager.live_node_count());
}

#[test]
fn handle_depleted_schedules_regrow_within_window() {
    let (mut manager, nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(3));
    let some_node = nodes.first().expect("world should have at least one node");
    let now_tick = 1_000;
    manager.handle_node_depleted(some_node.id, now_tick);
    let event = manager
        .regrow_queue
        .peek()
        .copied()
        .expect("regrow event should have been scheduled");
    let delay = event.fire_tick - now_tick;
    assert!(
        (MIN_REGROW_TICKS..=MAX_REGROW_TICKS).contains(&delay),
        "delay {delay} not in [{MIN_REGROW_TICKS}, {MAX_REGROW_TICKS}]"
    );
}

#[test]
fn tick_spawns_pending_regrows() {
    let (mut manager, mut nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(3));
    let initial_count = manager.live_node_count();
    let some_node = nodes.remove(0);
    let depleted_id = some_node.id;
    // Mirror the server's depletion: remove from the live map +
    // notify the manager.
    let mut existing: HashMap<ResourceNodeId, ResourceNodeState> =
        nodes.into_iter().map(|n| (n.id, n)).collect();
    manager.handle_node_depleted(depleted_id, 0);
    // Fast-forward past the maximum regrow window.
    let RegrowResult { spawned } = manager.tick(MAX_REGROW_TICKS + 1, &existing);
    // We should get back exactly one fresh spawn, same kind, fresh
    // position, new id.
    assert_eq!(spawned.len(), 1, "expected exactly one regrow");
    let fresh = &spawned[0];
    assert_ne!(fresh.id, depleted_id);
    existing.insert(fresh.id, fresh.clone());
    // Net live count: manager dropped one and replaced it.
    assert_eq!(manager.live_node_count(), initial_count);
}

#[test]
fn regrow_handles_the_meteorite_kind() {
    use crate::resource_nodes::METEORITE_NODE_ID;

    // A big world so some far rocky/ore chunks qualify for meteorite. The
    // generator + capacity grid share `chunk_kind_target`, so a capacity slot
    // marks a chunk the generator would have seeded meteorite in.
    let (mut manager, nodes) = ChunkManager::new_for_world(0x5EED_EA11, ChunkDims::new(25));
    let ember_coords: Vec<ChunkCoord> = manager
        .grids
        .iter()
        .filter(|(_, grid)| grid.capacity.contains_key(&NodeKind::Meteorite))
        .map(|(coord, _)| *coord)
        .collect();
    assert!(
        !ember_coords.is_empty(),
        "a 25x25 world should have at least one meteorite-eligible chunk"
    );

    // Worldgen actually fills (some of) those slots now that minerals are
    // exempt from the ring budget (the budget used to round every lone
    // meteorite away). Deplete the live ones so the regrow slots are open;
    // this also mirrors the real flow (a regrow always follows a depletion).
    let live_meteorites: Vec<ResourceNodeId> = nodes
        .iter()
        .filter(|state| state.definition_id == METEORITE_NODE_ID)
        .map(|state| state.id)
        .collect();
    assert!(
        !live_meteorites.is_empty(),
        "worldgen must seed at least one meteorite on a 25x25 world"
    );
    for id in live_meteorites {
        manager.handle_node_depleted(id, 0);
    }

    // Fire a meteorite regrow in each eligible chunk. Placement can legitimately
    // come up empty in a given chunk (the strict noise mask + capacity 1 rarity is
    // the point, and chunks whose node survives stay at cap), so we require that
    // ACROSS the eligible chunks at least one meteorite actually regrows, and
    // that every placement is a meteorite node tracked for the AoI system,
    // proving the kind flows through place_fresh_node and the capacity ceiling
    // like any other kind.
    let existing: HashMap<ResourceNodeId, ResourceNodeState> = HashMap::new();
    let mut total_placed = 0usize;
    for coord in ember_coords {
        manager.regrow_queue.push(RegrowEvent {
            fire_tick: 1,
            coord,
            kind: NodeKind::Meteorite,
        });
        let RegrowResult { spawned } = manager.tick(2, &existing);
        for state in &spawned {
            assert_eq!(
                state.definition_id, METEORITE_NODE_ID,
                "a meteorite regrow must place meteorite nodes only"
            );
            assert!(
                manager.node_chunks.contains_key(&state.id),
                "a regrown meteorite node must be tracked for AoI"
            );
        }
        total_placed += spawned.len();
    }
    assert!(
        total_placed >= 1,
        "at least one meteorite should regrow across the eligible chunks"
    );
}

#[test]
fn ring_budget_thins_clutter_but_never_deletes_minerals() {
    // Regression: `keep_n` ROUNDS, so at ring >= 3 (multiplier 0.45 and
    // below) a single-node group used to round to zero, which silently
    // deleted every stray/fringe iron node, lone stone vein, and worldgen
    // meteorite from the outer ~97% of the map. Minerals are exempt now;
    // the abundant clutter kinds still take the density falloff.
    let coord = ChunkCoord::new(7, 0); // ring 7 -> outermost multiplier (0.30)
    let mut next_id = 1u64;
    let mut spawn_of = |kind: NodeKind| {
        let id = next_id;
        next_id += 1;
        ChunkSpawn {
            coord,
            kind,
            spawn: crate::world::WorldResourceNodeSpawn::new(
                ResourceNodeId(id),
                kind.definition_id(),
                Vec3Net::new(id as f32 * 2.0, 0.0, 0.0),
                0.0,
            ),
        }
    };
    let mut spawns = Vec::new();
    for _ in 0..20 {
        spawns.push(spawn_of(NodeKind::HayGrass));
    }
    spawns.push(spawn_of(NodeKind::IronOre));
    spawns.push(spawn_of(NodeKind::Meteorite));
    spawns.push(spawn_of(NodeKind::StoneVein));
    spawns.push(spawn_of(NodeKind::SulfurOre));

    apply_ring_budget(&mut spawns);

    let count = |kind: NodeKind| spawns.iter().filter(|s| s.kind == kind).count();
    assert_eq!(
        count(NodeKind::HayGrass),
        6,
        "clutter still takes the outer-ring falloff (20 * 0.30)"
    );
    assert_eq!(count(NodeKind::IronOre), 1, "a lone iron node survives");
    assert_eq!(count(NodeKind::Meteorite), 1, "a meteorite survives");
    assert_eq!(count(NodeKind::StoneVein), 1, "a lone stone vein survives");
    assert_eq!(count(NodeKind::SulfurOre), 1, "a lone sulfur node survives");
}

#[test]
fn outer_ring_home_biomes_actually_hold_iron() {
    // End-to-end pin for the same regression: on a real fresh world, iron
    // spawned by the forest/plains stray rolls must survive into the live
    // node list OUTSIDE the centre 5x5 chunk block (ring >= 3), where the
    // old rounding deleted every single-node group. Summed across seeds so
    // one unlucky world can't flake the test.
    let dims = ChunkDims::new(15);
    let mut outer_home_iron = 0usize;
    for seed in [0xCAFEu64, 7, 42] {
        let (_manager, nodes) = ChunkManager::new_for_world(seed, dims);
        for state in &nodes {
            if state.definition_id != crate::resource_nodes::IRON_NODE_ID {
                continue;
            }
            let coord = ChunkCoord::from_world(state.position.x, state.position.z);
            let ring = coord.x.abs().max(coord.z.abs());
            if ring < 3 {
                continue;
            }
            let classification = ClassificationChannels::sample(seed, coord).classify();
            if matches!(
                classification,
                ChunkClassification::Forest | ChunkClassification::Plains
            ) {
                outer_home_iron += 1;
            }
        }
    }
    assert!(
        outer_home_iron > 0,
        "stray iron must survive the ring budget in outer forest/plains chunks"
    );
}

#[test]
fn view_tier_radius_is_monotonic() {
    assert!(view_tier_radius(ViewRadiusTier::Low) < view_tier_radius(ViewRadiusTier::Medium));
    assert!(view_tier_radius(ViewRadiusTier::Medium) < view_tier_radius(ViewRadiusTier::High));
}

#[test]
fn dropped_item_anchor_moves_when_position_crosses_chunk_boundary() {
    // Use a wider world so we can move across a chunk boundary
    // without falling off the playable map.
    let (mut manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
    let id: DroppedItemId = crate::protocol::DroppedItemId(42);

    manager.track_dropped_item(id, Vec3Net::new(8.0, 0.0, 0.0));
    let initial: Vec<_> = manager.dropped_items_in(ChunkCoord::new(0, 0)).collect();
    assert!(
        initial.contains(&id),
        "item should anchor in its origin chunk"
    );

    // 70m crosses into chunk x=1 (chunks are 64m wide).
    manager.update_dropped_item_chunk(id, Vec3Net::new(70.0, 0.0, 0.0));
    let after_origin: Vec<_> = manager.dropped_items_in(ChunkCoord::new(0, 0)).collect();
    let after_dest: Vec<_> = manager.dropped_items_in(ChunkCoord::new(1, 0)).collect();
    assert!(
        after_origin.is_empty(),
        "item must be removed from its old chunk after crossing the boundary"
    );
    assert!(
        after_dest.contains(&id),
        "item must appear in the new chunk after crossing the boundary"
    );

    manager.untrack_dropped_item(id);
    let after_untrack: Vec<_> = manager.dropped_items_in(ChunkCoord::new(1, 0)).collect();
    assert!(
        after_untrack.is_empty(),
        "untracking must drop the item from the chunk membership index"
    );
}

#[test]
fn player_anchor_follows_position_updates() {
    let (mut manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
    let client_id: ClientId = crate::protocol::ClientId(7);

    manager.track_player(client_id, Vec3Net::ZERO);
    assert_eq!(manager.player_chunk(client_id), Some(ChunkCoord::new(0, 0)));

    // 200m in +x and +z lands in chunk (3, 3).
    manager.update_player_chunk(client_id, Vec3Net::new(200.0, 0.0, 200.0));
    assert_eq!(
        manager.player_chunk(client_id),
        // 200/64 = 3.125 → floor → 3, but the test world is 5x5
        // (chunks -2..=2) so the out-of-bounds clamp pins it to 2.
        Some(ChunkCoord::new(2, 2))
    );

    manager.untrack_player(client_id);
    assert_eq!(manager.player_chunk(client_id), None);
}

#[test]
fn visible_chunks_centers_on_player_and_excludes_unloaded_coords() {
    let (manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(5));
    let visible_at_origin = manager.visible_chunks(Vec3Net::ZERO, ViewRadiusTier::Low);
    // Low tier + load buffer = radius 2; in a 5x5 world that's the
    // entire grid (chunks -2..=2).
    assert_eq!(visible_at_origin.len(), 25);

    // A player parked at the corner can only see chunks that exist.
    let corner = manager.visible_chunks(Vec3Net::new(128.0, 0.0, 128.0), ViewRadiusTier::Low);
    for coord in &corner {
        assert!(
            coord.x.abs() <= 2 && coord.z.abs() <= 2,
            "visible chunk {coord:?} is outside the loaded grid"
        );
    }
}

#[test]
fn retained_chunks_is_wider_superset_of_visible_chunks() {
    // World large enough that neither radius clamps against the edge.
    let (manager, _nodes) = ChunkManager::new_for_world(0xCAFE, ChunkDims::new(15));
    let visible = manager.visible_chunks(Vec3Net::ZERO, ViewRadiusTier::Low);
    let retained = manager.retained_chunks(Vec3Net::ZERO, ViewRadiusTier::Low);

    // Low tier(1) + load buffer(1) = radius 2 → 5x5; keep adds
    // KEEP_MARGIN_RINGS(2) → radius 4 → 9x9. The keep set must strictly
    // contain the add set, that gap is what gives the hysteresis.
    assert_eq!(visible.len(), 25);
    assert_eq!(retained.len(), 81);
    assert!(
        visible.is_subset(&retained),
        "every subscribed (visible) chunk must stay within the keep radius"
    );
}
