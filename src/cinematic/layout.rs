//! Hand-authored stage layout for the cinematic map.
//!
//! Everything here is static data: the pinned seed and dims of
//! `MapType::Cinematic`, the clear zones where procedural scatter is
//! suppressed, the exact resource-node placements inside those zones, the
//! pre-built base compound, the deployable props, and the dummy-actor roster.
//! The stage sits inside the ruin scatter's centre exclusion ring (15% of the
//! playable radius, about 71 m on the Small dims used here), so no ruin can
//! ever intersect a stage site regardless of the seed math.
//!
//! Coordinates are world-space metres on the flat world floor; `Vec2` fields
//! are `(x, z)` pairs at y = 0.

use bevy::math::Vec2;

use crate::building::{BuildingPiece, BuildingTier};
use crate::items::{
    BASIC_HATCHET_ID, HAMMER_ID, IRON_BOOTS_ID, IRON_CUIRASS_ID, IRON_GREAVES_ID, IRON_HATCHET_ID,
    IRON_HELM_ID, IRON_PICKAXE_ID, IRON_SWORD_ID, LAMELLAR_BOOTS_ID, LAMELLAR_GREAVES_ID,
    LAMELLAR_HELM_ID, LAMELLAR_VEST_ID, PADDED_HOOD_ID, PADDED_LEGGINGS_ID, PADDED_TUNIC_ID,
    PADDED_WRAPS_ID, STONE_SPEAR_ID,
};
use crate::protocol::{ResourceNodeId, Vec3Net};
use crate::world::chunk::{ChunkCoord, ChunkDims, ChunkSpawn, NodeKind};
use crate::world::ruins::RuinFootprint;
use crate::world::{ProceduralMapSize, WorldResourceNodeSpawn};

/// Pinned seed for every `MapType::Cinematic` world. The whole worldgen
/// pipeline is a pure function of `(seed, dims)`, so pinning both makes every
/// cinematic world byte-identical: terrain raster, biomes, ruins, and the
/// procedural scatter outside the stage zones all repeat exactly.
pub const CINEMATIC_SEED: u64 = 0xC14E_AA71_C14E_AA71;

/// Cinematic worlds are always Small (15 chunks, 960 m). Small keeps world
/// creation fast and the ruin exclusion ring still clears a ~71 m radius
/// around the origin, which is where the whole stage lives.
pub fn cinematic_dims() -> ChunkDims {
    ChunkDims::new(ProceduralMapSize::Small.dims())
}

/// A circular stage area where procedural node scatter (initial generation
/// AND regrow) is suppressed so the authored composition stays clean.
#[derive(Debug, Clone, Copy)]
pub struct StageZone {
    pub x: f32,
    pub z: f32,
    pub radius: f32,
}

/// Centre of the pre-built base compound.
pub const BASE_CENTER: Vec2 = Vec2::new(22.0, -14.0);
/// Centre of the gathering grove (authored trees + ore).
pub const GROVE_CENTER: Vec2 = Vec2::new(-26.0, 12.0);
/// Centre of the open PvP clearing.
pub const ARENA_CENTER: Vec2 = Vec2::new(2.0, 38.0);
/// Meteor impact point for the starfall shot. Far enough from every other
/// site that the blast (and the crater it leaves) touches nothing authored.
pub const METEOR_IMPACT: Vec2 = Vec2::new(-42.0, -38.0);
/// Blast size multiplier passed to the meteor event for the starfall shot.
pub const METEOR_SIZE: f32 = 1.0;

/// Where the admin who starts the cinematic is warped during the init phase.
/// Central so the whole stage sits inside their AoI chunk ring while the
/// detached camera flies around.
pub const PLAYER_ANCHOR: Vec2 = Vec2::new(2.0, 2.0);

pub const STAGE_ZONES: &[StageZone] = &[
    StageZone {
        x: BASE_CENTER.x,
        z: BASE_CENTER.y,
        radius: 16.0,
    },
    StageZone {
        x: GROVE_CENTER.x,
        z: GROVE_CENTER.y,
        radius: 16.0,
    },
    StageZone {
        x: ARENA_CENTER.x,
        z: ARENA_CENTER.y,
        radius: 14.0,
    },
    StageZone {
        x: METEOR_IMPACT.x,
        z: METEOR_IMPACT.y,
        // Wide enough to cover the sacrificial tree stand AND the survivor
        // trees just outside the blast radius.
        radius: 20.0,
    },
];

/// The stage zones as placement-exclusion footprints, the same shape the
/// ruin gate uses. The server hands these to the chunk manager so procedural
/// generation and later regrows both reject candidates inside a zone.
pub fn stage_exclusion_footprints() -> Vec<RuinFootprint> {
    STAGE_ZONES
        .iter()
        .map(|zone| RuinFootprint {
            x: zone.x,
            z: zone.z,
            radius: zone.radius,
        })
        .collect()
}

/// The hero pine the woodcutter chops during the harvest shot.
pub const GROVE_HERO_TREE: Vec2 = Vec2::new(-26.0, 12.0);
/// The stone vein the miner works.
pub const GROVE_STONE_VEIN: Vec2 = Vec2::new(-19.5, 17.5);
/// The iron deposit behind the stone vein.
pub const GROVE_IRON_NODE: Vec2 = Vec2::new(-17.0, 15.0);

/// Authored node placements: `(definition_id, x, z, yaw)`. These fill the
/// stage zones (which procedural scatter skips) with a composed set of trees,
/// ore, and clutter. Node ids are assigned by the caller after the procedural
/// range, so entries can be reordered or added freely.
pub fn authored_node_placements() -> &'static [(&'static str, f32, f32, f32)] {
    use crate::resource_nodes::{
        BIRCH_TREE_LARGE_NODE_ID, BIRCH_TREE_NODE_ID, BIRCH_TREE_SMALL_NODE_ID,
        BRANCH_PILE_NODE_ID, COAL_NODE_ID, HAY_GRASS_NODE_ID, IRON_NODE_ID,
        PINE_TREE_LARGE_NODE_ID, PINE_TREE_NODE_ID, PINE_TREE_SMALL_NODE_ID, STONE_NODE_ID,
        SURFACE_STONE_NODE_ID,
    };
    &[
        // --- GATHERING GROVE ---------------------------------------------
        // Hero pine front and centre; the woodcutter stands south of it so
        // the harvest orbit reads axe, tree, and the miner beyond.
        (PINE_TREE_LARGE_NODE_ID, GROVE_HERO_TREE.x, 12.0, 0.4),
        (PINE_TREE_LARGE_NODE_ID, -31.5, 8.0, 1.9),
        (PINE_TREE_NODE_ID, -29.0, 16.5, -0.7),
        (PINE_TREE_NODE_ID, -22.5, 8.5, 2.6),
        (BIRCH_TREE_LARGE_NODE_ID, -31.0, 14.5, 0.9),
        (BIRCH_TREE_NODE_ID, -23.0, 19.0, -1.5),
        (BIRCH_TREE_NODE_ID, -33.5, 11.0, 0.2),
        (PINE_TREE_SMALL_NODE_ID, -24.5, 15.5, 1.1),
        (BIRCH_TREE_SMALL_NODE_ID, -28.5, 9.0, -0.4),
        (PINE_TREE_SMALL_NODE_ID, -20.5, 11.5, 2.2),
        (BRANCH_PILE_NODE_ID, -27.5, 14.0, 0.7),
        (BRANCH_PILE_NODE_ID, -24.0, 10.0, -1.1),
        (SURFACE_STONE_NODE_ID, -22.0, 13.5, 0.3),
        (HAY_GRASS_NODE_ID, -25.0, 8.5, 0.0),
        (HAY_GRASS_NODE_ID, -29.5, 12.5, 0.8),
        // The mining corner: stone vein + iron + coal in one camera line.
        (STONE_NODE_ID, GROVE_STONE_VEIN.x, GROVE_STONE_VEIN.y, 0.6),
        (IRON_NODE_ID, GROVE_IRON_NODE.x, GROVE_IRON_NODE.y, -0.9),
        (COAL_NODE_ID, -21.5, 20.5, 1.4),
        (SURFACE_STONE_NODE_ID, -18.5, 18.5, -0.5),
        // --- BASE SURROUNDINGS -------------------------------------------
        // A loose treeline framing the compound without crowding the walls.
        (PINE_TREE_NODE_ID, 13.5, -20.5, 0.5),
        (BIRCH_TREE_NODE_ID, 31.0, -21.5, -1.2),
        (PINE_TREE_LARGE_NODE_ID, 33.0, -8.0, 2.0),
        (BIRCH_TREE_SMALL_NODE_ID, 14.0, -7.5, 0.9),
        (PINE_TREE_SMALL_NODE_ID, 30.5, -3.5, -0.3),
        (SURFACE_STONE_NODE_ID, 16.0, -16.5, 1.0),
        (BRANCH_PILE_NODE_ID, 15.0, -11.0, -0.7),
        (HAY_GRASS_NODE_ID, 17.5, -5.5, 0.2),
        (HAY_GRASS_NODE_ID, 29.0, -19.0, 1.3),
        // --- ARENA RING ---------------------------------------------------
        // The clearing stays open; hay tufts and a couple of small trees
        // ring the edge so the skirmish arc always has depth behind it.
        (BIRCH_TREE_NODE_ID, -8.5, 33.0, 0.4),
        (PINE_TREE_NODE_ID, 12.5, 33.5, -1.0),
        (PINE_TREE_SMALL_NODE_ID, -6.0, 44.0, 1.6),
        (BIRCH_TREE_SMALL_NODE_ID, 10.0, 45.0, -0.2),
        (HAY_GRASS_NODE_ID, -4.5, 38.5, 0.0),
        (HAY_GRASS_NODE_ID, 7.5, 40.5, 0.9),
        (HAY_GRASS_NODE_ID, 1.5, 45.5, -0.6),
        (HAY_GRASS_NODE_ID, 3.0, 31.5, 1.2),
        (SURFACE_STONE_NODE_ID, -2.5, 43.5, 0.5),
        (BRANCH_PILE_NODE_ID, 9.0, 36.0, -1.3),
        // --- METEOR FIELD -------------------------------------------------
        // A sacrificial tree stand INSIDE the blast radius (18 m at size
        // 1.0), so the starfall impact visibly fells trees on camera, plus
        // survivors just outside it for contrast. Stones sell the emptiness
        // at ground zero.
        (PINE_TREE_NODE_ID, -52.0, -44.0, 0.8),
        (PINE_TREE_LARGE_NODE_ID, -49.5, -30.0, -0.5),
        (BIRCH_TREE_NODE_ID, -34.0, -46.0, 1.7),
        (PINE_TREE_SMALL_NODE_ID, -31.5, -33.0, 0.2),
        (PINE_TREE_NODE_ID, -56.5, -49.0, -1.1),
        (BIRCH_TREE_NODE_ID, -28.5, -47.5, 0.6),
        (SURFACE_STONE_NODE_ID, -47.0, -33.0, 0.7),
        (SURFACE_STONE_NODE_ID, -37.5, -43.0, -0.4),
        (HAY_GRASS_NODE_ID, -44.5, -42.5, 1.1),
        (HAY_GRASS_NODE_ID, -36.5, -34.0, 0.3),
    ]
}

/// Turn the authored placement table into chunk spawns, assigning dense node
/// ids starting at `first_id`. Every definition id in the table maps to a
/// `NodeKind` (asserted in tests), so chunk bookkeeping (capacity, regrow,
/// AoI membership) treats authored nodes exactly like procedural ones.
pub fn authored_chunk_spawns(first_id: u64) -> Vec<ChunkSpawn> {
    authored_node_placements()
        .iter()
        .enumerate()
        .filter_map(|(index, (definition_id, x, z, yaw))| {
            let kind = NodeKind::from_definition_id(definition_id)?;
            Some(ChunkSpawn {
                coord: ChunkCoord::from_world(*x, *z),
                kind,
                spawn: WorldResourceNodeSpawn::new(
                    ResourceNodeId(first_id + index as u64),
                    *definition_id,
                    Vec3Net::new(*x, 0.0, *z),
                    *yaw,
                ),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Base compound
// ---------------------------------------------------------------------------

/// Which edge of a foundation cell a wall-like piece stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellEdge {
    North,
    South,
    East,
    West,
}

/// One authored building block, expressed on the base's local foundation
/// grid so the layout stays readable. `cell` is a `(col, row)` pair; col
/// grows +x, row grows +z, cell `(0, 0)`'s centre sits at [`BASE_ORIGIN`].
/// `edge` is `Some` for wall-like pieces (which stand on a platform edge)
/// and `None` for platforms (foundations at ground level, ceilings on top
/// of the walls).
#[derive(Debug, Clone, Copy)]
pub struct StageBuildingBlock {
    pub piece: BuildingPiece,
    pub tier: BuildingTier,
    pub cell: (i32, i32),
    pub edge: Option<CellEdge>,
    /// 0 = ground storey, 1 = one wall-height up (ceilings, upper walls).
    pub level: u8,
}

/// World position of the centre of base grid cell `(0, 0)`.
pub const BASE_ORIGIN: Vec2 = Vec2::new(20.5, -15.5);

const fn platform(
    piece: BuildingPiece,
    tier: BuildingTier,
    cell: (i32, i32),
    level: u8,
) -> StageBuildingBlock {
    StageBuildingBlock {
        piece,
        tier,
        cell,
        edge: None,
        level,
    }
}

const fn wall(
    piece: BuildingPiece,
    tier: BuildingTier,
    cell: (i32, i32),
    edge: CellEdge,
) -> StageBuildingBlock {
    StageBuildingBlock {
        piece,
        tier,
        cell,
        edge: Some(edge),
        level: 0,
    }
}

/// The pre-built base: a 2x2 hewn-wood cabin with a doorway on the south
/// face, and ceilings for a roof. The east extension is deliberately NOT
/// here: the builder erects it live during the homestead shot (see
/// [`homestead_build_sequence`]).
pub fn base_building_blocks() -> Vec<StageBuildingBlock> {
    use BuildingPiece::{Ceiling, Doorway, Foundation, Wall, WindowWall};
    use BuildingTier::HewnWood;
    use CellEdge::{East, North, South, West};
    vec![
        // 2x2 foundation slab.
        platform(Foundation, HewnWood, (0, 0), 0),
        platform(Foundation, HewnWood, (1, 0), 0),
        platform(Foundation, HewnWood, (0, 1), 0),
        platform(Foundation, HewnWood, (1, 1), 0),
        // Perimeter walls. South face carries the doorway (camera-facing);
        // the west face gets a window wall so interior torchlight spills out.
        wall(Wall, HewnWood, (0, 0), North),
        wall(Wall, HewnWood, (1, 0), North),
        wall(WindowWall, HewnWood, (0, 0), West),
        wall(Wall, HewnWood, (0, 1), West),
        wall(Doorway, HewnWood, (0, 1), South),
        wall(Wall, HewnWood, (1, 1), South),
        wall(Wall, HewnWood, (1, 0), East),
        wall(Wall, HewnWood, (1, 1), East),
        // Roof.
        platform(Ceiling, HewnWood, (0, 0), 1),
        platform(Ceiling, HewnWood, (1, 0), 1),
        platform(Ceiling, HewnWood, (0, 1), 1),
        platform(Ceiling, HewnWood, (1, 1), 1),
    ]
}

/// One step of the homestead live build-out: the extension the builder
/// erects ON CAMERA during the homestead shot (it is deliberately absent
/// from [`base_building_blocks`], so the shot shows real construction:
/// pieces appearing under hammer swings, an upgrade changing material, and
/// deployables going down).
#[derive(Debug, Clone, Copy)]
pub enum BuildStep {
    /// Place a building block.
    Block(StageBuildingBlock),
    /// Upgrade the block at `(cell, edge)` to `tier` (its model changes).
    Upgrade {
        cell: (i32, i32),
        edge: Option<CellEdge>,
        tier: BuildingTier,
    },
    /// Place a deployable prop (the sleeping bag on the fresh floor).
    Prop(StageProp),
}

#[derive(Debug, Clone, Copy)]
pub struct TimedBuildStep {
    /// Seconds into the homestead shot this step lands (with a hammer swing).
    pub at_seconds: f32,
    pub step: BuildStep,
}

pub fn homestead_build_sequence() -> Vec<TimedBuildStep> {
    use BuildingPiece::{Foundation, Wall};
    use BuildingTier::{HewnWood, Sticks};
    use CellEdge::{East, North};
    vec![
        TimedBuildStep {
            at_seconds: 0.9,
            step: BuildStep::Block(platform(Foundation, Sticks, (2, 0), 0)),
        },
        TimedBuildStep {
            at_seconds: 2.8,
            step: BuildStep::Block(platform(Foundation, Sticks, (2, 1), 0)),
        },
        TimedBuildStep {
            at_seconds: 4.6,
            step: BuildStep::Block(wall(Wall, Sticks, (2, 0), North)),
        },
        TimedBuildStep {
            at_seconds: 6.4,
            step: BuildStep::Block(wall(Wall, Sticks, (2, 0), East)),
        },
        TimedBuildStep {
            at_seconds: 8.4,
            step: BuildStep::Upgrade {
                cell: (2, 0),
                edge: Some(North),
                tier: HewnWood,
            },
        },
        TimedBuildStep {
            at_seconds: 10.4,
            step: BuildStep::Prop(StageProp {
                kind: StagePropKind::SleepingBag,
                x: 26.5,
                y: crate::building::FOUNDATION_HEIGHT_M,
                z: -12.5,
                yaw: 0.9,
            }),
        },
        TimedBuildStep {
            at_seconds: 12.2,
            step: BuildStep::Prop(StageProp {
                kind: StagePropKind::Furnace,
                x: 30.2,
                y: 0.0,
                z: -12.2,
                yaw: -0.7,
            }),
        },
    ]
}

/// Deployable props placed during the init phase, outside the building grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagePropKind {
    WorkbenchT1,
    Furnace,
    ToolCupboard,
    StorageBoxSmall,
    SleepingBag,
    TorchGround,
    /// Wall-mounted torch; yaw points out of the wall face.
    TorchWall,
}

#[derive(Debug, Clone, Copy)]
pub struct StageProp {
    pub kind: StagePropKind,
    pub x: f32,
    /// Base height. Props inside the cabin stand on the foundation top
    /// (`FOUNDATION_HEIGHT_M`); outdoor props sit on the ground; wall
    /// torches mount partway up the wall face.
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
}

pub const STAGE_PROPS: &[StageProp] = &[
    // Inside the cabin, standing on the foundation slab.
    StageProp {
        kind: StagePropKind::ToolCupboard,
        x: 20.6,
        y: crate::building::FOUNDATION_HEIGHT_M,
        z: -16.2,
        yaw: 0.0,
    },
    StageProp {
        kind: StagePropKind::WorkbenchT1,
        x: 23.4,
        y: crate::building::FOUNDATION_HEIGHT_M,
        z: -16.2,
        yaw: 3.1,
    },
    StageProp {
        kind: StagePropKind::StorageBoxSmall,
        x: 20.5,
        y: crate::building::FOUNDATION_HEIGHT_M,
        z: -13.0,
        yaw: 1.6,
    },
    StageProp {
        kind: StagePropKind::SleepingBag,
        x: 22.6,
        y: crate::building::FOUNDATION_HEIGHT_M,
        z: -13.2,
        yaw: 0.4,
    },
    // Outside: the furnace burns by the south-west corner so its smoke and
    // glow read in the homestead and emberlight shots.
    StageProp {
        kind: StagePropKind::Furnace,
        x: 17.8,
        y: 0.0,
        z: -18.6,
        yaw: 0.8,
    },
    StageProp {
        kind: StagePropKind::TorchGround,
        x: 19.4,
        y: 0.0,
        z: -19.8,
        yaw: 0.0,
    },
    StageProp {
        kind: StagePropKind::TorchGround,
        x: 25.2,
        y: 0.0,
        z: -19.2,
        yaw: 0.0,
    },
    // Door-side torch pair on the south (camera-facing) wall, lighting the
    // entrance for the night shot. Positioned just outside the wall face
    // plane at z = -11.0, partway up the wall; yaw 0 tilts the torch out
    // along +z (the outward wall normal), matching the client's wall-mount
    // tilt convention in `deployable_visual_transform`.
    StageProp {
        kind: StagePropKind::TorchWall,
        x: 19.3,
        y: 1.6,
        z: -10.85,
        yaw: 0.0,
    },
    StageProp {
        kind: StagePropKind::TorchWall,
        x: 23.5,
        y: 1.6,
        z: -10.85,
        yaw: 0.0,
    },
];

// ---------------------------------------------------------------------------
// Actors
// ---------------------------------------------------------------------------

/// Worn armor per paperdoll slot: `[head, chest, legs, feet]`.
pub type ArmorLoadout = [Option<&'static str>; 4];

/// What a dummy actor does while the cinematic runs. The server orchestrator
/// turns each role into per-tick pose / swing writes on a synthetic player.
#[derive(Debug, Clone, Copy)]
pub enum ActorRole {
    /// Stand at a node and swing at it on a work cadence.
    Chopper { node: Vec2 },
    /// Alternate between two nearby nodes, walking between them.
    Miner { nodes: [Vec2; 2] },
    /// Face a building block and swing the hammer at it.
    Builder { target: Vec2 },
    /// Circle the arena centre and trade swings with the other fighter.
    /// `dies` marks the loser of the scripted exchange.
    Fighter { arena: Vec2, dies: bool },
    /// Walk a looping waypoint route through the stage.
    Wanderer { waypoints: &'static [Vec2] },
}

#[derive(Debug, Clone, Copy)]
pub struct ActorSpec {
    pub name: &'static str,
    pub spawn: Vec2,
    pub yaw: f32,
    pub held_item: &'static str,
    pub armor: ArmorLoadout,
    pub role: ActorRole,
}

/// Waypoint loop for the wandering actor: a wide circuit that passes the
/// grove, the arena edge, and the base front so most shots catch a figure
/// moving somewhere in the middle distance.
pub const WANDER_ROUTE: &[Vec2] = &[
    Vec2::new(8.0, -2.0),
    Vec2::new(-10.0, 6.0),
    Vec2::new(-18.0, 22.0),
    Vec2::new(-6.0, 32.0),
    Vec2::new(12.0, 24.0),
    Vec2::new(24.0, 4.0),
    Vec2::new(16.0, -8.0),
];

pub const STAGE_ACTORS: &[ActorSpec] = &[
    ActorSpec {
        name: "Bram",
        spawn: Vec2::new(-26.0, 14.2),
        yaw: std::f32::consts::PI,
        held_item: IRON_HATCHET_ID,
        armor: [None, Some(PADDED_TUNIC_ID), None, Some(PADDED_WRAPS_ID)],
        role: ActorRole::Chopper {
            node: GROVE_HERO_TREE,
        },
    },
    ActorSpec {
        name: "Sigrid",
        spawn: Vec2::new(-20.0, 19.5),
        yaw: -1.2,
        held_item: IRON_PICKAXE_ID,
        armor: [Some(PADDED_HOOD_ID), Some(PADDED_TUNIC_ID), None, None],
        role: ActorRole::Miner {
            nodes: [GROVE_STONE_VEIN, GROVE_IRON_NODE],
        },
    },
    ActorSpec {
        name: "Tove",
        spawn: Vec2::new(29.4, -14.0),
        yaw: std::f32::consts::FRAC_PI_2,
        held_item: HAMMER_ID,
        armor: [None, Some(LAMELLAR_VEST_ID), None, Some(LAMELLAR_BOOTS_ID)],
        role: ActorRole::Builder {
            // Ambient target: the cabin's east wall face, so off-shot hammer
            // work lands on a real structure (repair taps). During the
            // homestead shot the build sequence overrides this with the
            // live extension steps.
            target: Vec2::new(25.0, -14.0),
        },
    },
    ActorSpec {
        name: "Kell",
        spawn: Vec2::new(-1.5, 34.5),
        yaw: 0.6,
        held_item: IRON_SWORD_ID,
        armor: [
            Some(IRON_HELM_ID),
            Some(IRON_CUIRASS_ID),
            Some(IRON_GREAVES_ID),
            Some(IRON_BOOTS_ID),
        ],
        role: ActorRole::Fighter {
            arena: ARENA_CENTER,
            dies: false,
        },
    },
    ActorSpec {
        name: "Runa",
        spawn: Vec2::new(5.5, 41.5),
        yaw: -2.5,
        held_item: STONE_SPEAR_ID,
        armor: [
            Some(LAMELLAR_HELM_ID),
            Some(LAMELLAR_VEST_ID),
            Some(LAMELLAR_GREAVES_ID),
            None,
        ],
        role: ActorRole::Fighter {
            arena: ARENA_CENTER,
            dies: true,
        },
    },
    ActorSpec {
        name: "Aldis",
        spawn: Vec2::new(8.0, -2.0),
        yaw: 2.4,
        held_item: BASIC_HATCHET_ID,
        armor: [
            Some(PADDED_HOOD_ID),
            Some(PADDED_TUNIC_ID),
            Some(PADDED_LEGGINGS_ID),
            None,
        ],
        role: ActorRole::Wanderer {
            waypoints: WANDER_ROUTE,
        },
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::PlayableBounds;

    fn bounds() -> PlayableBounds {
        PlayableBounds::from_dims(cinematic_dims())
    }

    fn inside_a_zone(x: f32, z: f32) -> bool {
        STAGE_ZONES.iter().any(|zone| {
            let dx = x - zone.x;
            let dz = z - zone.z;
            dx * dx + dz * dz <= zone.radius * zone.radius
        })
    }

    #[test]
    fn stage_zones_sit_in_the_ruin_free_centre() {
        // The whole stage relies on the ruin scatter's centre exclusion ring:
        // every zone CENTRE must sit inside it (so the authored sites can
        // never collide with a ruin shell), and every zone must fit inside
        // the playable interior.
        let bounds = bounds();
        let exclusion =
            bounds.max_x.max(1.0) * crate::game_balance::RUIN_SPAWN_EXCLUSION_RADIUS_FRACTION;
        for zone in STAGE_ZONES {
            let centre_dist = (zone.x * zone.x + zone.z * zone.z).sqrt();
            assert!(
                centre_dist < exclusion,
                "zone at ({}, {}) is outside the ruin exclusion ring",
                zone.x,
                zone.z
            );
            assert!(bounds.contains(zone.x - zone.radius, zone.z - zone.radius));
            assert!(bounds.contains(zone.x + zone.radius, zone.z + zone.radius));
        }
    }

    #[test]
    fn authored_placements_resolve_and_stay_inside_stage_zones() {
        let bounds = bounds();
        for (definition_id, x, z, _yaw) in authored_node_placements() {
            assert!(
                NodeKind::from_definition_id(definition_id).is_some(),
                "{definition_id} does not map to a NodeKind"
            );
            assert!(
                crate::resource_nodes::resource_node_definition(definition_id).is_some(),
                "{definition_id} is not a registered resource node"
            );
            assert!(
                bounds.contains(*x, *z),
                "({x}, {z}) outside playable bounds"
            );
            assert!(
                inside_a_zone(*x, *z),
                "authored node at ({x}, {z}) is outside every stage clear zone \
                 (procedural scatter would crowd it)"
            );
        }
    }

    #[test]
    fn authored_chunk_spawns_assign_dense_ids_and_matching_coords() {
        let spawns = authored_chunk_spawns(500);
        assert_eq!(spawns.len(), authored_node_placements().len());
        for (index, spawn) in spawns.iter().enumerate() {
            assert_eq!(spawn.spawn.id.0, 500 + index as u64);
            assert_eq!(
                spawn.coord,
                ChunkCoord::from_world(spawn.spawn.position.x, spawn.spawn.position.z)
            );
        }
    }

    #[test]
    fn actor_specs_resolve_items_and_spawn_in_bounds() {
        let bounds = bounds();
        let mut fighters = 0;
        let mut deaths = 0;
        for spec in STAGE_ACTORS {
            let held = crate::items::item_definition(&crate::items::intern_item_id(spec.held_item));
            assert!(held.is_some(), "{} holds unknown item", spec.name);
            assert!(
                held.is_some_and(|definition| definition.equipable),
                "{} holds an unequipable item (no held mesh would render)",
                spec.name
            );
            for piece in spec.armor.iter().flatten() {
                let definition =
                    crate::items::item_definition(&crate::items::intern_item_id(piece));
                assert!(
                    definition.is_some_and(|definition| definition.armor.is_some()),
                    "{} wears {piece}, which is not armor",
                    spec.name
                );
            }
            assert!(bounds.contains(spec.spawn.x, spec.spawn.y));
            if let ActorRole::Fighter { dies, .. } = spec.role {
                fighters += 1;
                if dies {
                    deaths += 1;
                }
            }
            if let ActorRole::Wanderer { waypoints } = spec.role {
                assert!(waypoints.len() >= 2);
                for point in waypoints {
                    assert!(bounds.contains(point.x, point.y));
                }
            }
        }
        assert_eq!(fighters, 2, "the skirmish choreography needs exactly two");
        assert_eq!(deaths, 1, "exactly one fighter falls");
    }

    #[test]
    fn stage_props_resolve_to_deployables() {
        for prop in STAGE_PROPS {
            let item_id = match prop.kind {
                StagePropKind::WorkbenchT1 => crate::items::WORKBENCH_T1_ID,
                StagePropKind::Furnace => crate::items::CRUDE_FURNACE_ID,
                StagePropKind::ToolCupboard => crate::items::TOOL_CUPBOARD_ID,
                StagePropKind::StorageBoxSmall => crate::items::STORAGE_BOX_SMALL_ID,
                StagePropKind::SleepingBag => crate::items::SLEEPING_BAG_ID,
                StagePropKind::TorchGround | StagePropKind::TorchWall => crate::items::TORCH_ID,
            };
            let definition = crate::items::item_definition(&crate::items::intern_item_id(item_id));
            assert!(
                definition.is_some_and(|definition| definition.deployable.is_some()),
                "prop {item_id} has no deployable profile"
            );
        }
    }

    #[test]
    fn base_grid_is_coherent() {
        let blocks = base_building_blocks();
        // Exactly one doorway (one door spawns per doorway).
        let doorways = blocks
            .iter()
            .filter(|block| matches!(block.piece, BuildingPiece::Doorway))
            .count();
        assert_eq!(doorways, 1);
        // Every wall-like piece stands on a cell that carries a platform at
        // its storey (foundation at level 0), so stability holds.
        for block in &blocks {
            if block.edge.is_some() {
                assert!(
                    blocks.iter().any(|platform| {
                        platform.edge.is_none()
                            && platform.cell == block.cell
                            && ((block.level == 0
                                && matches!(platform.piece, BuildingPiece::Foundation))
                                || (block.level > 0
                                    && matches!(platform.piece, BuildingPiece::Ceiling)
                                    && platform.level == block.level))
                    }),
                    "wall at cell {:?} level {} has no supporting platform",
                    block.cell,
                    block.level
                );
            }
        }
    }
}
