//! Raid-economics tests: the Phase 6 exit criterion. These drive real charges
//! through `resolve_explosion` end to end and assert the spec's raid math holds,
//! plus the fuse / fizzle / thrown-bomb / self-damage / claim / save-round-trip
//! behaviours the systems core promises.
//!
//! The blast centre is placed inside the target wall's footprint so
//! `nearest_surface_distance` is 0 and the linear falloff is 1.0, i.e. the
//! per-charge damage equals `base * effectiveness_pct / 100`, exactly the
//! point-blank numbers in the spec table. That makes the "N charges break it,
//! N-1 do not" assertions test the effectiveness matrix and the wall HP
//! together, which is the phase exit criterion.

use super::*;
use crate::{
    building::{BuildingPiece, BuildingTier},
    items::{
        DeployableKind, DoorVariant, ExplosiveKind, POWDER_BOMB_ID, POWDER_KEG_ID,
        SATCHEL_CHARGE_ID, intern_item_id,
    },
    protocol::{
        AccountId, DeployedEntityId, GAME_VERSION, PROTOCOL_VERSION, PlaceDeployableCommand,
        Vec3Net,
    },
    server::test_support::{connect_host, server},
};

/// Connect a client on a specific account id (so a raider is a distinct account
/// from the base owner), pinned to origin. Local to this module.
fn connect_account(server: &mut GameServer, account: AccountId, name: &str) -> ClientId {
    let client_id = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            account,
            name.to_owned(),
            String::new(),
        )
        .expect("client should connect")
        .0;
    server
        .clients
        .get_mut(&client_id)
        .expect("connected client")
        .controller
        .position = Vec3Net::ZERO;
    client_id
}

/// Place a building wall at `position` at a specific tier (bypassing the hammer
/// upgrade path), so a test has a wall at a known HP + material to blast.
/// Returns the id.
fn place_wall(server: &mut GameServer, position: Vec3Net, tier: BuildingTier) -> DeployedEntityId {
    let piece = BuildingPiece::Wall;
    let max_health = crate::building::building_max_health(piece, tier);
    let id = server.next_deployed_entity_id;
    server.next_deployed_entity_id.0 += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id,
        item_id: intern_item_id(crate::building::building_item_id(piece)),
        kind: DeployableKind::Building { piece, tier },
        position,
        yaw: 0.0,
        health: max_health,
        max_health,
        owner: Some(crate::protocol::AccountId(1)),
        furnace: None,
        placed_at_tick: 0,
        door: None,
        label: None,
        stability: 100,
        storage: None,
        torch: None,
        cupboard: None,
        ruin_cache: None,
        fuse: None,
    };
    server.insert_deployed_entity(id, entity);
    server.chunk_manager.track_deployed_entity(id, position);
    id
}

/// Place a free-standing furnace at `position` (a blast witness that the
/// stability sweep never cascade-destroys, unlike an unsupported wall). Returns
/// the id.
fn witness_furnace(server: &mut GameServer, position: Vec3Net) -> DeployedEntityId {
    let item_id = intern_item_id(crate::items::CRUDE_FURNACE_ID);
    let max_health = crate::items::item_definition(&item_id)
        .and_then(|d| d.deployable)
        .map(|p| p.max_health)
        .expect("furnace has a deployable profile");
    let id = server.next_deployed_entity_id;
    server.next_deployed_entity_id.0 += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id,
        item_id,
        kind: DeployableKind::Furnace { tier: 1 },
        position,
        yaw: 0.0,
        health: max_health,
        max_health,
        owner: Some(crate::protocol::AccountId(1)),
        furnace: None,
        placed_at_tick: 0,
        door: None,
        label: None,
        stability: 100,
        storage: None,
        torch: None,
        cupboard: None,
        ruin_cache: None,
        fuse: None,
    };
    server.insert_deployed_entity(id, entity);
    server.chunk_manager.track_deployed_entity(id, position);
    id
}

/// Place an iron door entity directly (bypassing the doorway-mount path). Returns
/// the id. Iron doors are the metal raid material at 3,000 HP.
fn place_iron_door(server: &mut GameServer, position: Vec3Net) -> DeployedEntityId {
    let variant = DoorVariant::Iron;
    let max_health = variant.max_hp();
    let id = server.next_deployed_entity_id;
    server.next_deployed_entity_id.0 += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id,
        item_id: intern_item_id(variant.item_id()),
        kind: DeployableKind::Door { variant },
        position,
        yaw: 0.0,
        health: max_health,
        max_health,
        owner: Some(crate::protocol::AccountId(1)),
        furnace: None,
        placed_at_tick: 0,
        door: None,
        label: None,
        stability: 100,
        storage: None,
        torch: None,
        cupboard: None,
        ruin_cache: None,
        fuse: None,
    };
    server.insert_deployed_entity(id, entity);
    server.chunk_manager.track_deployed_entity(id, position);
    id
}

/// Detonate `count` charges of `kind` at the target's centre (falloff 1.0), one
/// after another, resolving each blast fully. Returns whether the target
/// survived (`Some(remaining_hp)`) or was destroyed (`None`).
fn blast_n_times(
    server: &mut GameServer,
    center: Vec3Net,
    kind: ExplosiveKind,
    count: u32,
    target: DeployedEntityId,
) -> Option<u32> {
    for _ in 0..count {
        // Stop early if the target is already gone (a later blast would just be a
        // no-op, but bail so the return reads cleanly as "destroyed").
        server.deployed_entities.get(&target)?;
        let _ = server.resolve_explosion(center, kind);
    }
    server.deployed_entities.get(&target).map(|e| e.health)
}

#[test]
fn five_kegs_break_a_hewn_wood_wall_and_four_do_not() {
    // A hewn-wood wall is 3,600 HP; a keg does 900 * 80% = 720 per point-blank
    // hit, so 4 kegs (2,880) leave it standing and 5 (3,600) break it.
    let mut four = server();
    let wall = place_wall(&mut four, Vec3Net::ZERO, BuildingTier::HewnWood);
    let remaining = blast_n_times(&mut four, Vec3Net::ZERO, ExplosiveKind::PowderKeg, 4, wall);
    assert_eq!(
        remaining,
        Some(3_600 - 4 * 720),
        "4 kegs must NOT break a hewn wood wall (2,880 of 3,600)"
    );

    let mut five = server();
    let wall = place_wall(&mut five, Vec3Net::ZERO, BuildingTier::HewnWood);
    let remaining = blast_n_times(&mut five, Vec3Net::ZERO, ExplosiveKind::PowderKeg, 5, wall);
    assert_eq!(remaining, None, "5 kegs must break a hewn wood wall");
}

#[test]
fn seven_satchels_break_a_stone_wall_and_six_do_not() {
    // A stone wall is 6,000 HP; a satchel does 2,000 * 45% = 900 per hit,
    // so 6 (5,400) leave it up and 7 (6,300) break it.
    let mut six = server();
    let wall = place_wall(&mut six, Vec3Net::ZERO, BuildingTier::Stone);
    let remaining = blast_n_times(
        &mut six,
        Vec3Net::ZERO,
        ExplosiveKind::SatchelCharge,
        6,
        wall,
    );
    assert_eq!(
        remaining,
        Some(6_000 - 6 * 900),
        "6 satchels must NOT break a stone wall (5,400 of 6,000)"
    );

    let mut seven = server();
    let wall = place_wall(&mut seven, Vec3Net::ZERO, BuildingTier::Stone);
    let remaining = blast_n_times(
        &mut seven,
        Vec3Net::ZERO,
        ExplosiveKind::SatchelCharge,
        7,
        wall,
    );
    assert_eq!(remaining, None, "7 satchels must break a stone wall");
}

#[test]
fn a_satchel_does_exactly_eight_percent_of_base_vs_metal() {
    // Satchel base 2,000, metal effectiveness 8% => 160 per hit. Assert the door
    // lost exactly that after one point-blank satchel.
    let mut server = server();
    let door = place_iron_door(&mut server, Vec3Net::ZERO);
    let remaining = blast_n_times(
        &mut server,
        Vec3Net::ZERO,
        ExplosiveKind::SatchelCharge,
        1,
        door,
    );
    assert_eq!(
        remaining,
        Some(3_000 - 160),
        "one satchel must do exactly 8% of base (160) to metal"
    );
}

#[test]
fn a_bomb_cannot_damage_metal_at_all() {
    // Powder bomb metal effectiveness is 0%: an iron door is bomb-proof no matter
    // how many land.
    let mut server = server();
    let door = place_iron_door(&mut server, Vec3Net::ZERO);
    let remaining = blast_n_times(
        &mut server,
        Vec3Net::ZERO,
        ExplosiveKind::PowderBomb,
        10,
        door,
    );
    assert_eq!(
        remaining,
        Some(3_000),
        "a powder bomb must do nothing to metal, even 10 of them"
    );
}

#[test]
fn a_single_bomb_shreds_a_sticks_wall() {
    // Sticks-tier wall is 250 HP; a bomb does 300 * 100% = 300 at point blank, so
    // one bomb tears it down. Sticks structures shred to single bombs.
    let mut server = server();
    let wall = place_wall(&mut server, Vec3Net::ZERO, BuildingTier::Sticks);
    let remaining = blast_n_times(
        &mut server,
        Vec3Net::ZERO,
        ExplosiveKind::PowderBomb,
        1,
        wall,
    );
    assert_eq!(remaining, None, "one powder bomb must shred a sticks wall");
}

#[test]
fn placing_a_charge_arms_its_fuse_and_it_detonates_at_zero() {
    let mut server = server();
    let host = connect_host(&mut server);
    server
        .clients
        .get_mut(&host)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(crate::protocol::ItemStack::new(POWDER_KEG_ID, 1));

    // Place a keg on the ground in front of the host. The placement arms the fuse.
    server.apply_place_deployable_command(
        host,
        PlaceDeployableCommand {
            item_id: intern_item_id(POWDER_KEG_ID),
            position: Vec3Net::new(1.0, 0.0, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    let charge_id = server
        .deployed_entities
        .values()
        .find(|e| matches!(e.kind, DeployableKind::Explosive { .. }))
        .map(|e| e.id)
        .expect("the keg placed as an armed explosive charge");
    let fuse_ticks = server.deployed_entities[&charge_id]
        .fuse
        .as_ref()
        .expect("a placed charge is armed")
        .ticks_left;
    assert_eq!(
        fuse_ticks,
        crate::game_balance::POWDER_KEG_FUSE_TICKS,
        "placement arms the full keg fuse"
    );

    // Tick the fuse to one before zero: still present.
    for _ in 0..(fuse_ticks - 1) {
        let _ = server.tick_fuses();
    }
    assert!(
        server.deployed_entities.contains_key(&charge_id),
        "the charge is still armed one tick before detonation"
    );
    // The last tick detonates and removes it.
    let _ = server.tick_fuses();
    assert!(
        !server.deployed_entities.contains_key(&charge_id),
        "the charge detonated and was removed at fuse zero"
    );
}

#[test]
fn damaging_a_charge_to_zero_fizzles_it_without_detonating() {
    let mut server = server();
    // A free-standing furnace right next to a charge, so if the charge DETONATED
    // it would damage the furnace; if it FIZZLED the furnace stays pristine. A
    // furnace (not a wall) is used so the stability sweep can't cascade-destroy an
    // unsupported building piece and confuse the assertion.
    let witness = witness_furnace(&mut server, Vec3Net::new(0.5, 0.0, 0.0));
    let witness_hp = server.deployed_entities[&witness].health;

    // Arm a keg at the origin directly.
    let charge = server.next_deployed_entity_id;
    server.next_deployed_entity_id.0 += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id: charge,
        item_id: intern_item_id(POWDER_KEG_ID),
        kind: DeployableKind::Explosive {
            kind: ExplosiveKind::PowderKeg,
        },
        position: Vec3Net::ZERO,
        yaw: 0.0,
        health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
        max_health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
        owner: Some(crate::protocol::AccountId(1)),
        furnace: None,
        placed_at_tick: 0,
        door: None,
        label: None,
        stability: 100,
        storage: None,
        torch: None,
        cupboard: None,
        ruin_cache: None,
        fuse: Some(crate::server::fuse::FuseState::armed(
            crate::game_balance::POWDER_KEG_FUSE_TICKS,
        )),
    };
    server.insert_deployed_entity(charge, entity);
    server
        .chunk_manager
        .track_deployed_entity(charge, Vec3Net::ZERO);

    // A raider with a hatchet chops the charge out (cloth material: any tool
    // shreds it). Drive the deployable damage path until the charge is gone.
    let host = connect_host(&mut server);
    server
        .clients
        .get_mut(&host)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(crate::protocol::ItemStack::new(BASIC_HATCHET_ID, 1));
    for _ in 0..20 {
        if server.deployed_entities.get(&charge).is_none() {
            break;
        }
        server.clients.get_mut(&host).unwrap().next_gather_tick = 0;
        let _ = server.apply_damage_deployable_command(
            host,
            crate::protocol::DamageDeployableCommand { id: charge },
        );
    }

    assert!(
        server.deployed_entities.get(&charge).is_none(),
        "the charge was shot/chopped out"
    );
    // The key assertion: a fizzle does NOT detonate, so the adjacent furnace is
    // untouched (a detonation would have chipped it).
    assert_eq!(
        server.deployed_entities[&witness].health, witness_hp,
        "fizzling a charge must not detonate: the adjacent furnace is pristine"
    );
}

#[test]
fn a_thrown_bomb_bounces_rolls_and_detonates_where_it_stops() {
    let mut server = server();
    let host = connect_host(&mut server);
    server
        .clients
        .get_mut(&host)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(crate::protocol::ItemStack::new(POWDER_BOMB_ID, 3));

    // Throw the bomb forward at full power over open ground.
    let _ = server.apply_explosive_command(
        host,
        crate::protocol::ExplosiveCommand::Throw {
            aim_dir: Vec3Net::new(1.0, 0.0, 0.0),
            power: 1.0,
        },
    );
    // One bomb consumed.
    assert_eq!(
        crate::inventory::count_items_in_inventory(
            &server.clients[&host].inventory,
            POWDER_BOMB_ID
        ),
        2,
        "throwing consumes exactly one bomb"
    );

    // The bomb lives its whole life as a projectile: it never converts into a
    // placed deployable. Tick it through flight/bounce/roll and record its
    // travel; when the fuse runs out it must detonate (projectile removed) and
    // it must never have created an `Explosive` deployable along the way.
    let bomb_id = *server
        .projectiles
        .keys()
        .next()
        .expect("the thrown bomb is a live projectile");
    let mut ticks = 0u32;
    let mut came_to_rest = false;
    while server.projectiles.get(&bomb_id).is_some() {
        assert!(
            !server
                .deployed_entities
                .values()
                .any(|e| matches!(e.kind, DeployableKind::Explosive { .. })),
            "a thrown bomb must never attach as a placed deployable"
        );
        if let Some(p) = server.projectiles.get(&bomb_id)
            && p.velocity == Vec3Net::ZERO
        {
            came_to_rest = true;
        }
        let _ = server.tick_projectiles(1.0 / crate::protocol::SERVER_TICK_RATE_HZ);
        server.tick += 1;
        ticks += 1;
        assert!(ticks < 400, "the bomb must detonate when its fuse expires");
    }
    assert!(
        came_to_rest,
        "a full-power lob over open ground bounces and rolls to a rest before the fuse blows"
    );
    // Fuse length matches the balance constant (from the throw, not from rest).
    assert_eq!(
        ticks,
        crate::game_balance::POWDER_BOMB_FUSE_TICKS,
        "the fuse burns from the moment of the throw"
    );
}

#[test]
fn throw_power_scales_launch_speed_within_the_clamp() {
    // Full power launches at the max speed; a forged tiny/NaN power clamps to
    // the min-charge floor, never slower.
    let mut fast = server();
    let host = connect_host(&mut fast);
    fast.clients
        .get_mut(&host)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(crate::protocol::ItemStack::new(POWDER_BOMB_ID, 1));
    let _ = fast.apply_explosive_command(
        host,
        crate::protocol::ExplosiveCommand::Throw {
            aim_dir: Vec3Net::new(1.0, 0.0, 0.0),
            power: 1.0,
        },
    );
    let v = fast.projectiles.values().next().unwrap().velocity;
    let speed = v.length_squared().sqrt();
    assert!(
        (speed - crate::game_balance::POWDER_BOMB_MAX_THROW_SPEED_MPS).abs() < 1e-3,
        "full power throws at max speed, got {speed}"
    );

    let mut weak = server();
    let host = connect_host(&mut weak);
    weak.clients
        .get_mut(&host)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(crate::protocol::ItemStack::new(POWDER_BOMB_ID, 1));
    let _ = weak.apply_explosive_command(
        host,
        crate::protocol::ExplosiveCommand::Throw {
            aim_dir: Vec3Net::new(1.0, 0.0, 0.0),
            power: f32::NAN,
        },
    );
    let v = weak.projectiles.values().next().unwrap().velocity;
    let speed = v.length_squared().sqrt();
    let floor = crate::game_balance::POWDER_BOMB_MIN_THROW_SPEED_MPS
        + (crate::game_balance::POWDER_BOMB_MAX_THROW_SPEED_MPS
            - crate::game_balance::POWDER_BOMB_MIN_THROW_SPEED_MPS)
            * crate::game_balance::POWDER_BOMB_MIN_THROW_FRACTION;
    assert!(
        (speed - floor).abs() < 1e-3,
        "a non-finite power clamps to the min-charge floor speed, got {speed}"
    );
}

#[test]
fn self_damage_applies_to_a_player_at_ground_zero() {
    // Resolved question 3: own-charge self-damage counts. A player standing on the
    // blast centre (no armor) takes the full ground-zero blast.
    let mut server = server();
    let host = connect_host(&mut server); // pinned to origin
    let full = server.clients[&host].controller.health;

    let _ = server.resolve_explosion(Vec3Net::ZERO, ExplosiveKind::PowderKeg);
    let after = server.clients[&host].controller.health;
    assert!(
        after < full,
        "a player at ground zero of their own blast takes damage ({after} < {full})"
    );
}

#[test]
fn explosion_respects_the_blast_armor_column() {
    use crate::items::{IRON_BOOTS_ID, IRON_CUIRASS_ID, IRON_GREAVES_ID, IRON_HELM_ID};
    use crate::protocol::{EquipmentSlot, ItemStack};

    // Two identical players (pinned to origin), one bare, one in a full iron set
    // (blast column 20%). The blast centre is placed ~2.5 m away so the falloff
    // brings the raw hit into a survivable band on both, making the armor
    // difference visible rather than masked by a shared lethal ground-zero hit.
    let off_center = Vec3Net::new(2.5, 0.0, 0.0);

    let mut bare = server();
    let a = connect_host(&mut bare);
    let bare_full = bare.clients[&a].controller.health;
    let _ = bare.resolve_explosion(off_center, ExplosiveKind::PowderBomb);
    let bare_taken = bare_full - bare.clients[&a].controller.health;
    assert!(bare_taken > 0.0, "the bare player took some blast");

    let mut armored = server();
    let b = connect_host(&mut armored);
    {
        let slots = &mut armored
            .clients
            .get_mut(&b)
            .unwrap()
            .inventory
            .equipment_slots;
        slots[EquipmentSlot::Head.index()] = Some(ItemStack::new(IRON_HELM_ID, 1));
        slots[EquipmentSlot::Chest.index()] = Some(ItemStack::new(IRON_CUIRASS_ID, 1));
        slots[EquipmentSlot::Legs.index()] = Some(ItemStack::new(IRON_GREAVES_ID, 1));
        slots[EquipmentSlot::Feet.index()] = Some(ItemStack::new(IRON_BOOTS_ID, 1));
    }
    // Recompute protection from the freshly-equipped set.
    let protection =
        crate::items::equipped_protection(&armored.clients[&b].inventory.equipment_slots);
    armored.clients.get_mut(&b).unwrap().protection = protection;
    assert_eq!(protection.blast, 20, "full iron set is 20% blast");

    let armored_full = armored.clients[&b].controller.health;
    let _ = armored.resolve_explosion(off_center, ExplosiveKind::PowderBomb);
    let armored_taken = armored_full - armored.clients[&b].controller.health;

    assert!(
        armored_taken < bare_taken,
        "iron blast armor reduces the hit ({armored_taken} < {bare_taken})"
    );
}

#[test]
fn placing_a_charge_is_allowed_inside_an_enemy_claim() {
    use crate::protocol::AccountId;
    use crate::server::test_support::place_foundation;
    // Owner (account 1) claims a base; a raider (account 2) must still be able to
    // place a charge inside it (that is the point of raiding). A furnace, by
    // contrast, would be blocked, which is the control for this test.
    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO);

    // Owner places a cupboard on the foundation to claim the base.
    let owner = connect_host(&mut server); // account 1
    let top = crate::building::platform_top_offset(BuildingPiece::Foundation).unwrap();
    server
        .clients
        .get_mut(&owner)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(crate::protocol::ItemStack::new(
        crate::items::TOOL_CUPBOARD_ID,
        1,
    ));
    server.apply_place_deployable_command(
        owner,
        PlaceDeployableCommand {
            item_id: intern_item_id(crate::items::TOOL_CUPBOARD_ID),
            position: Vec3Net::new(0.0, top, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    assert!(
        !server.claim_footprints.is_empty(),
        "the owner's cupboard projects a claim"
    );

    // A raider (account 2, NOT authorized) connects and stands on the claim.
    let raider = connect_account(&mut server, AccountId(2), "Raider");
    server.clients.get_mut(&raider).unwrap().controller.position = Vec3Net::ZERO;
    server
        .clients
        .get_mut(&raider)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(crate::protocol::ItemStack::new(POWDER_KEG_ID, 1));

    // Placing the keg on the claimed foundation succeeds despite the enemy claim.
    server.apply_place_deployable_command(
        raider,
        PlaceDeployableCommand {
            item_id: intern_item_id(POWDER_KEG_ID),
            position: Vec3Net::new(0.0, top, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    assert!(
        server
            .deployed_entities
            .values()
            .any(|e| matches!(e.kind, DeployableKind::Explosive { .. })),
        "a raider must be able to place a charge inside an enemy claim"
    );
}

#[test]
fn an_armed_charge_survives_a_save_round_trip_with_its_fuse() {
    let mut server = server();
    // Arm a satchel charge with a partial fuse.
    let charge = server.next_deployed_entity_id;
    server.next_deployed_entity_id.0 += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id: charge,
        item_id: intern_item_id(SATCHEL_CHARGE_ID),
        kind: DeployableKind::Explosive {
            kind: ExplosiveKind::SatchelCharge,
        },
        position: Vec3Net::new(5.0, 0.0, 5.0),
        yaw: 0.0,
        health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
        max_health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
        owner: Some(crate::protocol::AccountId(7)),
        furnace: None,
        placed_at_tick: 0,
        door: None,
        label: None,
        stability: 100,
        storage: None,
        torch: None,
        cupboard: None,
        ruin_cache: None,
        fuse: Some(crate::server::fuse::FuseState::armed(123)),
    };
    server.insert_deployed_entity(charge, entity);

    // Round-trip through the persist -> restore mapping.
    let persisted = server.persisted_deployed_entities();
    let restored = GameServer::restore_deployed_entities(persisted);
    let restored_charge = restored
        .values()
        .find(|e| matches!(e.kind, DeployableKind::Explosive { .. }))
        .expect("the armed charge survived the round trip");
    assert_eq!(
        restored_charge.fuse.as_ref().map(|f| f.ticks_left),
        Some(123),
        "the fuse countdown resumes from where it was saved"
    );
    // The specific charge kind survives too.
    assert!(matches!(
        restored_charge.kind,
        DeployableKind::Explosive {
            kind: ExplosiveKind::SatchelCharge
        }
    ));
}

/// Arm a keg charge directly at `position` owned by `owner`, tracked so the
/// interaction path can find it. Returns its id. Local defuse-test helper.
fn arm_keg(server: &mut GameServer, position: Vec3Net, owner: AccountId) -> DeployedEntityId {
    let id = server.next_deployed_entity_id;
    server.next_deployed_entity_id.0 += 1;
    let entity = crate::server::deployables::DeployedEntity {
        id,
        item_id: intern_item_id(POWDER_KEG_ID),
        kind: DeployableKind::Explosive {
            kind: ExplosiveKind::PowderKeg,
        },
        position,
        yaw: 0.0,
        health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
        max_health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
        owner: Some(owner),
        furnace: None,
        placed_at_tick: 0,
        door: None,
        label: None,
        stability: 100,
        storage: None,
        torch: None,
        cupboard: None,
        ruin_cache: None,
        fuse: Some(crate::server::fuse::FuseState::armed(
            crate::game_balance::POWDER_KEG_FUSE_TICKS,
        )),
    };
    server.insert_deployed_entity(id, entity);
    server.chunk_manager.track_deployed_entity(id, position);
    id
}

#[test]
fn anyone_can_defuse_a_charge_outside_any_claim() {
    use crate::protocol::ExplosiveCommand;
    let mut server = server();
    // A witness furnace right next to the charge: if defusing DETONATED the
    // charge it would take damage; a clean defuse leaves it pristine.
    let witness = witness_furnace(&mut server, Vec3Net::new(0.5, 0.0, 0.0));
    let witness_hp = server.deployed_entities[&witness].health;

    // The charge sits on unclaimed ground, owned by some raider (account 9).
    let charge = arm_keg(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(9));

    // A random passer-by (the host, account 1, not the owner) stands on it and
    // defuses it. Outside any claim, anyone in reach may defuse.
    let defender = connect_host(&mut server);
    server
        .clients
        .get_mut(&defender)
        .unwrap()
        .controller
        .position = Vec3Net::ZERO;

    let envelopes = server.defuse_charge(defender, charge);

    assert!(
        !server.deployed_entities.contains_key(&charge),
        "the charge was removed by the defuse"
    );
    assert_eq!(
        server.deployed_entities[&witness].health, witness_hp,
        "a defused charge never detonates, so the witness is untouched"
    );
    // Half the keg recipe refunded: 30 gunpowder -> 15, 15 wood -> 7, 2 twine -> 1.
    let inv = &server.clients[&defender].inventory;
    assert_eq!(
        crate::inventory::count_items_in_inventory(inv, crate::items::GUNPOWDER_ID),
        15,
        "half the gunpowder is refunded to the defuser"
    );
    assert_eq!(
        crate::inventory::count_items_in_inventory(inv, crate::items::WOOD_ID),
        7
    );
    assert_eq!(
        crate::inventory::count_items_in_inventory(inv, crate::items::PLANT_TWINE_ID),
        1
    );
    // Success toast to the defuser.
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.message,
            crate::protocol::ServerMessage::Toast(t)
                if t.kind == crate::protocol::ToastKind::Success
        )),
        "a successful defuse toasts the defender"
    );
    let _ = ExplosiveCommand::Defuse { id: charge }; // command exists on the wire
}

#[test]
fn an_unauthorized_player_cannot_defuse_a_charge_inside_a_claim() {
    use crate::server::test_support::place_foundation;
    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO);

    // Owner (account 1) claims the base with a cupboard on the foundation.
    let owner = connect_host(&mut server);
    let top = crate::building::platform_top_offset(BuildingPiece::Foundation).unwrap();
    server
        .clients
        .get_mut(&owner)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(crate::protocol::ItemStack::new(
        crate::items::TOOL_CUPBOARD_ID,
        1,
    ));
    server.apply_place_deployable_command(
        owner,
        PlaceDeployableCommand {
            item_id: intern_item_id(crate::items::TOOL_CUPBOARD_ID),
            position: Vec3Net::new(0.0, top, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    assert!(
        !server.claim_footprints.is_empty(),
        "owner's cupboard claims the base"
    );

    // A raider's charge is armed inside the claim.
    let charge = arm_keg(
        &mut server,
        Vec3Net::new(0.0, top, 0.0),
        crate::protocol::AccountId(2),
    );

    // An unauthorized outsider (account 3) stands on the claim and tries to defuse.
    let outsider = connect_account(&mut server, AccountId(3), "Outsider");
    server
        .clients
        .get_mut(&outsider)
        .unwrap()
        .controller
        .position = Vec3Net::new(0.0, top, 0.0);

    let envelopes = server.defuse_charge(outsider, charge);

    assert!(
        server.deployed_entities.contains_key(&charge),
        "an unauthorized player's defuse inside a claim is rejected, the charge stays"
    );
    assert_eq!(
        crate::inventory::count_items_in_inventory(
            &server.clients[&outsider].inventory,
            crate::items::GUNPOWDER_ID
        ),
        0,
        "a rejected defuse refunds nothing"
    );
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.message,
            crate::protocol::ServerMessage::Toast(t)
                if t.kind == crate::protocol::ToastKind::Warning
        )),
        "a rejected defuse warns the requester"
    );
}

#[test]
fn an_authorized_player_can_defuse_a_charge_inside_a_claim() {
    use crate::protocol::ClaimCommand;
    use crate::server::test_support::place_foundation;
    let mut server = server();
    place_foundation(&mut server, Vec3Net::ZERO);

    let owner = connect_host(&mut server);
    let top = crate::building::platform_top_offset(BuildingPiece::Foundation).unwrap();
    server
        .clients
        .get_mut(&owner)
        .unwrap()
        .inventory
        .actionbar_slots[0] = Some(crate::protocol::ItemStack::new(
        crate::items::TOOL_CUPBOARD_ID,
        1,
    ));
    server.apply_place_deployable_command(
        owner,
        PlaceDeployableCommand {
            item_id: intern_item_id(crate::items::TOOL_CUPBOARD_ID),
            position: Vec3Net::new(0.0, top, 0.0),
            yaw: 0.0,
            wall_mounted: false,
        },
    );
    let cupboard = server
        .deployed_entities
        .values()
        .find(|e| matches!(e.kind, DeployableKind::ToolCupboard))
        .map(|e| e.id)
        .expect("cupboard placed");
    // The owner (account 1) is auto-authorized on their own cupboard.
    assert!(server.cupboard_authorizes(cupboard, crate::protocol::AccountId(1)));

    // A raider's charge armed inside the claim.
    let charge = arm_keg(
        &mut server,
        Vec3Net::new(0.0, top, 0.0),
        crate::protocol::AccountId(2),
    );

    // The owner stands on the claim and defuses their attacker's charge.
    server.clients.get_mut(&owner).unwrap().controller.position = Vec3Net::new(0.0, top, 0.0);
    let _ = ClaimCommand::AuthorizeSelf { id: cupboard }; // already authorized as placer
    let envelopes = server.defuse_charge(owner, charge);

    assert!(
        !server.deployed_entities.contains_key(&charge),
        "an authorized defender defuses a charge inside their own claim"
    );
    assert_eq!(
        crate::inventory::count_items_in_inventory(
            &server.clients[&owner].inventory,
            crate::items::GUNPOWDER_ID
        ),
        15,
        "the authorized defuser recovers half the materials"
    );
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.message,
            crate::protocol::ServerMessage::Toast(t)
                if t.kind == crate::protocol::ToastKind::Success
        )),
        "the authorized defuse succeeds and toasts"
    );
}

#[test]
fn a_defused_charge_never_detonates() {
    let mut server = server();
    // Two witness furnaces straddling the charge; a detonation would damage both.
    let w1 = witness_furnace(&mut server, Vec3Net::new(0.5, 0.0, 0.0));
    let w2 = witness_furnace(&mut server, Vec3Net::new(-0.5, 0.0, 0.0));
    let hp1 = server.deployed_entities[&w1].health;
    let hp2 = server.deployed_entities[&w2].health;

    let charge = arm_keg(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(9));
    let defender = connect_host(&mut server);
    server
        .clients
        .get_mut(&defender)
        .unwrap()
        .controller
        .position = Vec3Net::ZERO;

    let _ = server.defuse_charge(defender, charge);
    // Ticking the fuse subsystem afterwards must not detonate the removed charge.
    for _ in 0..crate::game_balance::POWDER_KEG_FUSE_TICKS + 1 {
        let _ = server.tick_fuses();
    }
    assert_eq!(server.deployed_entities[&w1].health, hp1);
    assert_eq!(server.deployed_entities[&w2].health, hp2);
}

#[test]
fn defuse_overflow_drops_when_the_bag_is_full() {
    let mut server = server();
    let charge = arm_keg(&mut server, Vec3Net::ZERO, crate::protocol::AccountId(9));
    let defender = connect_host(&mut server);
    server
        .clients
        .get_mut(&defender)
        .unwrap()
        .controller
        .position = Vec3Net::ZERO;

    // Jam every inventory + actionbar slot with a non-stacking-with-refund item so
    // the gunpowder / wood / twine refund cannot land and must drop instead.
    {
        let inv = &mut server.clients.get_mut(&defender).unwrap().inventory;
        for slot in inv.inventory_slots.iter_mut() {
            *slot = Some(crate::protocol::ItemStack::new(crate::items::STONE_ID, 1));
        }
        for slot in inv.actionbar_slots.iter_mut() {
            *slot = Some(crate::protocol::ItemStack::new(crate::items::STONE_ID, 1));
        }
    }
    let dropped_before = server.dropped_items_iter().count();

    let _ = server.defuse_charge(defender, charge);

    assert!(
        !server.deployed_entities.contains_key(&charge),
        "the charge is still removed even when the refund cannot fit"
    );
    // None of the refund landed in the (full) bag.
    assert_eq!(
        crate::inventory::count_items_in_inventory(
            &server.clients[&defender].inventory,
            crate::items::GUNPOWDER_ID
        ),
        0,
        "a full bag takes none of the refund"
    );
    assert!(
        server.dropped_items_iter().count() > dropped_before,
        "the refund the bag could not hold dropped as world items"
    );
}
