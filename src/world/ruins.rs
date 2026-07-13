//! Burnt-out houses: deterministic point-of-interest worldgen.
//!
//! A ruin is a small hand-authored burnt-house prefab (a cottage, farmhouse,
//! shed, or barn gutted by a meteor strike), scattered across the world at
//! generation time and carrying one to three restocking salvage chests. The
//! world's story is written in these: the meteor storm the players live under
//! has been falling for a while, and the houses it hit are what's left to
//! scavenge. No lost civilisation, nothing arcane; just homes that burnt.
//! Everything in this module is a **pure function of `(world_seed, dims)`**,
//! exactly like the resource-node spawn pipeline, so the server (worldgen +
//! chest spawning) and the client (map glyphs, shell meshes) compute the
//! identical layout with no wire traffic and no divergence risk
//! (singleplayer == multiplayer).
//!
//! The load-bearing entry point is [`ruin_layout`]: same seed, same sites;
//! different seeds, different sites. From a layout you can derive:
//!
//! - [`ruin_footprints`] for the node-rejection gate in the chunk generator
//!   (nodes must not spawn inside a ruin),
//! - [`RuinSite::static_blocks`] for the collision/LoS geometry registered as
//!   world blocks (the perimeter-wall precedent),
//! - [`RuinSite::cache_points`] for the world-space chest spawn positions, and
//! - the site transform (`x`, `z`, [`RuinSite::yaw`]) for the client's shell
//!   mesh spawn (one authored glb per prefab, see `assets/ruins/`).
//!
//! Placement (in [`ruin_layout`]) is Poisson-style rejection sampling seeded
//! from the world seed: candidates are drawn across the playable interior,
//! rejected if inside the centre exclusion ring, outside the bounds margin, or
//! within `RUIN_MIN_SPACING_M` of an already-accepted site.
//!
//! Sites rotate in exact quarter turns only. The collision pipeline is
//! AABB-only ([`WorldBlock`] has no yaw), so a quarter-turn grid keeps every
//! collider box exactly coincident with the authored shell; an arbitrary yaw
//! would force loose enclosing boxes that block doorways and clip walls.

use crate::{
    game_balance::{
        RUIN_BOUNDS_MARGIN_M, RUIN_MIN_SPACING_M, RUIN_SCATTER_CANDIDATES,
        RUIN_SPAWN_EXCLUSION_RADIUS_FRACTION,
    },
    protocol::Vec3Net,
    world::{
        WorldBlock,
        chunk::{ChunkDims, PlayableBounds, splitmix64},
    },
};

/// The four hand-authored burnt-house prefabs. Appended-only; the discriminant
/// is never persisted (a ruin layout is recomputed from the seed on every
/// load), so ordering is free to change, but keep it stable for readable
/// tests. Each maps to one authored shell glb (`assets/ruins/<stem>.glb`,
/// built by `art/ruins/build_ruins.py`) plus the collider boxes below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuinPrefab {
    /// A one-room home: back wall mostly standing, the rest burnt to stubs,
    /// a door gap in the front wall. One chest against the back wall.
    BurntCottage,
    /// The largest shell: an L-shaped main room + annex, one tall gable end
    /// still up. Two chests (main room and annex).
    BurntFarmhouse,
    /// A three-sided outbuilding, open across the front. One chest.
    BurntShed,
    /// A wide barn: two gable ends standing, long walls burnt low, a cart
    /// opening in one long side. Two chests.
    BurntBarn,
}

/// One axis-aligned collider box in prefab-local space: `center` is the box
/// centre (y included, so a wall box sits on the plinth top), `half` the half
/// extents. Rotated into world space by the site's quarter-turn yaw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuinBox {
    pub center: (f32, f32, f32),
    pub half: (f32, f32, f32),
}

impl RuinBox {
    const fn new(center: (f32, f32, f32), half: (f32, f32, f32)) -> Self {
        Self { center, half }
    }
}

impl RuinPrefab {
    /// Every prefab, in a stable order. The scatter picks from this by index.
    pub const ALL: [Self; 4] = [
        Self::BurntCottage,
        Self::BurntFarmhouse,
        Self::BurntShed,
        Self::BurntBarn,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::BurntCottage => "Burnt Cottage",
            Self::BurntFarmhouse => "Burnt Farmhouse",
            Self::BurntShed => "Burnt Shed",
            Self::BurntBarn => "Burnt Barn",
        }
    }

    /// Asset stem of the prefab's shell glb: `assets/ruins/<stem>.glb`.
    pub const fn asset_stem(self) -> &'static str {
        match self {
            Self::BurntCottage => "burnt_cottage",
            Self::BurntFarmhouse => "burnt_farmhouse",
            Self::BurntShed => "burnt_shed",
            Self::BurntBarn => "burnt_barn",
        }
    }

    /// Stable dense index (0..[`RuinPrefab::ALL`]`.len()`), for asset arrays.
    pub const fn index(self) -> usize {
        match self {
            Self::BurntCottage => 0,
            Self::BurntFarmhouse => 1,
            Self::BurntShed => 2,
            Self::BurntBarn => 3,
        }
    }

    /// The stone floor plinths the house stands on. Solid, walkable at
    /// `FLOOR_TOP_M` (the building-foundation height, so the movement step-up
    /// that handles foundations handles these too). Separate from
    /// [`RuinPrefab::wall_boxes`] because the chest points must sit over a
    /// plinth (pinned by test).
    pub fn plinth_boxes(self) -> &'static [RuinBox] {
        match self {
            Self::BurntCottage => COTTAGE_PLINTHS,
            Self::BurntFarmhouse => FARMHOUSE_PLINTHS,
            Self::BurntShed => SHED_PLINTHS,
            Self::BurntBarn => BARN_PLINTHS,
        }
    }

    /// The standing wall stubs (charred plank walls at their surviving
    /// heights). Door and cart openings are genuine gaps between boxes, so
    /// they stay passable. Matched by hand to the authored shell geometry in
    /// `art/ruins/build_ruins.py`; edit the two together.
    pub fn wall_boxes(self) -> &'static [RuinBox] {
        match self {
            Self::BurntCottage => COTTAGE_WALLS,
            Self::BurntFarmhouse => FARMHOUSE_WALLS,
            Self::BurntShed => SHED_WALLS,
            Self::BurntBarn => BARN_WALLS,
        }
    }

    /// Prefab-local chest spawn points (1 to 3 per prefab). Each is a local
    /// XZ offset from the site centre; every point lies over a plinth
    /// (pinned by test), and [`RuinSite::cache_points`] spawns the chest at
    /// the plinth TOP so it sits proud on the floor.
    pub fn cache_points(self) -> &'static [(f32, f32)] {
        match self {
            Self::BurntCottage => &[(0.9, -1.3)],
            Self::BurntFarmhouse => &[(-2.2, -1.5), (4.5, 1.2)],
            Self::BurntShed => &[(-0.5, -0.6)],
            Self::BurntBarn => &[(-2.6, 0.0), (2.4, 1.0)],
        }
    }

    /// Radius, in metres, of the site's circular footprint. Node spawns inside
    /// this circle are rejected, so it is sized to enclose the whole prefab
    /// plus a little clearance. A generous single circle is far cheaper to test
    /// per node than a per-element polygon and reads the same in practice.
    pub const fn footprint_radius_m(self) -> f32 {
        match self {
            Self::BurntCottage => 6.0,
            Self::BurntFarmhouse => 8.5,
            Self::BurntShed => 4.5,
            Self::BurntBarn => 7.5,
        }
    }
}

/// One scattered ruin: which prefab, where (world XZ), and its orientation in
/// exact quarter turns (see the module docs for why not arbitrary yaw). The
/// ground height is `y = 0` (the world floor is flat); this stays implicit so
/// a future heightmap can snap it without changing the shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuinSite {
    pub prefab: RuinPrefab,
    pub x: f32,
    pub z: f32,
    /// Orientation in quarter turns about +Y, `0..=3`.
    pub quarter_turns: u8,
}

/// Height of the walkable stone floor plinth top, and therefore the chest
/// spawn height. Matches the building-foundation height on purpose: the
/// movement code already steps players up onto foundation tops, so plinths
/// walk identically.
pub const FLOOR_TOP_M: f32 = crate::building::FOUNDATION_HEIGHT_M;

impl RuinSite {
    /// World-space centre of the site as a [`Vec3Net`] at ground level.
    pub fn center(self) -> Vec3Net {
        Vec3Net::new(self.x, 0.0, self.z)
    }

    /// The site yaw in radians (quarter turns about +Y), for render
    /// transforms. Collider math never touches this; it rotates exactly via
    /// [`rotate_quarter`].
    pub fn yaw(self) -> f32 {
        f32::from(self.quarter_turns) * std::f32::consts::FRAC_PI_2
    }

    /// Rotate a prefab-local XZ offset by the site's quarter turns into a
    /// world offset, exactly (no trig, no float drift at the cardinals).
    fn local_to_world(self, local_x: f32, local_z: f32) -> (f32, f32) {
        let (rx, rz) = rotate_quarter(self.quarter_turns, local_x, local_z);
        (self.x + rx, self.z + rz)
    }

    /// World-space chest spawn positions, at the plinth top so a chest sits
    /// proud on the floor rather than half-embedded in it.
    pub fn cache_points(self) -> Vec<Vec3Net> {
        self.prefab
            .cache_points()
            .iter()
            .map(|&(lx, lz)| {
                let (wx, wz) = self.local_to_world(lx, lz);
                Vec3Net::new(wx, FLOOR_TOP_M, wz)
            })
            .collect()
    }

    /// Static collision/LoS blocks for the whole site, in world space. These
    /// are registered as [`WorldBlock`]s at world build (the perimeter-wall
    /// precedent), so the `BlockGrid` gives collision, projectile LoS, and
    /// melee LoS for free. Because sites rotate in quarter turns only, every
    /// box here is exactly the authored geometry, not a loose enclosure.
    pub fn static_blocks(self) -> Vec<WorldBlock> {
        self.prefab
            .plinth_boxes()
            .iter()
            .chain(self.prefab.wall_boxes())
            .map(|b| {
                let (cx, cy, cz) = b.center;
                let (hx, hy, hz) = b.half;
                let (wx, wz) = self.local_to_world(cx, cz);
                let (rhx, rhz) = rotate_quarter_half_extents(self.quarter_turns, hx, hz);
                WorldBlock::ruin(Vec3Net::new(wx, cy, wz), Vec3Net::new(rhx, hy, rhz))
            })
            .collect()
    }
}

/// Rotate an XZ offset by `quarter_turns` quarter turns about +Y. MUST match
/// Bevy's `Quat::from_rotation_y(yaw)` at the cardinals (`x' = x cos + z sin`,
/// `z' = -x sin + z cos`), the same convention as `rotate_offset` in
/// `src/building.rs`, because the client renders the shell mesh with exactly
/// that quaternion at [`RuinSite::yaw`]. The opposite spin here once rotated
/// the colliders 180 degrees against the visuals on q=1/q=3 sites (an
/// invisible wall standing in the visible doorway); pinned by test against
/// `building::rotate_offset` semantics.
fn rotate_quarter(quarter_turns: u8, x: f32, z: f32) -> (f32, f32) {
    match quarter_turns % 4 {
        0 => (x, z),
        1 => (z, -x),
        2 => (-x, -z),
        _ => (-z, x),
    }
}

/// Half extents swap between X and Z on odd quarter turns.
fn rotate_quarter_half_extents(quarter_turns: u8, hx: f32, hz: f32) -> (f32, f32) {
    if quarter_turns.is_multiple_of(2) {
        (hx, hz)
    } else {
        (hz, hx)
    }
}

/// A ruin's circular footprint in world space, used to reject resource-node
/// candidates that would spawn inside a ruin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuinFootprint {
    pub x: f32,
    pub z: f32,
    pub radius: f32,
}

impl RuinFootprint {
    pub fn contains(self, x: f32, z: f32) -> bool {
        let dx = x - self.x;
        let dz = z - self.z;
        dx * dx + dz * dz <= self.radius * self.radius
    }
}

/// The footprints of every site in a layout, for the node-rejection gate.
pub fn ruin_footprints(sites: &[RuinSite]) -> Vec<RuinFootprint> {
    sites
        .iter()
        .map(|site| RuinFootprint {
            x: site.x,
            z: site.z,
            radius: site.prefab.footprint_radius_m(),
        })
        .collect()
}

/// True if `(x, z)` falls inside any ruin footprint. The chunk generator calls
/// this per node candidate; at a few dozen ruins it is a cheap linear scan.
pub fn point_in_any_footprint(footprints: &[RuinFootprint], x: f32, z: f32) -> bool {
    footprints.iter().any(|fp| fp.contains(x, z))
}

/// True if `(x, z)` falls inside any ruin footprint inflated by `margin`
/// metres. The player-placement gate (server validation and the client
/// ghost) tests against `RUIN_PLACEMENT_EXCLUSION_MARGIN_M`, so nobody can
/// wall in a salvage chest or camp a restock with a sleeping bag.
pub fn point_near_any_footprint(footprints: &[RuinFootprint], x: f32, z: f32, margin: f32) -> bool {
    footprints.iter().any(|fp| {
        let dx = x - fp.x;
        let dz = z - fp.z;
        let reach = fp.radius + margin;
        dx * dx + dz * dz <= reach * reach
    })
}

/// **The** shared, deterministic ruin scatter. A pure function of
/// `(world_seed, dims)`: the server calls it at world build (for static blocks,
/// chest spawns, and the node-rejection footprints) and the client calls it for
/// the map glyphs and shell meshes, so both agree with zero coordination.
///
/// Placement is Poisson-style rejection sampling: draw `RUIN_SCATTER_CANDIDATES`
/// candidate points across the playable interior from a seeded stream, and keep
/// a candidate only when it is
///
/// - outside the centre exclusion ring
///   (`RUIN_SPAWN_EXCLUSION_RADIUS_FRACTION` of the playable radius),
/// - inside `PlayableBounds` shrunk by `RUIN_BOUNDS_MARGIN_M`, and
/// - at least `RUIN_MIN_SPACING_M` from every already-accepted site.
///
/// Each accepted site draws its prefab and orientation from the same seeded
/// stream, so the whole layout round-trips identically.
pub fn ruin_layout(world_seed: u64, dims: ChunkDims) -> Vec<RuinSite> {
    let bounds = PlayableBounds::from_dims(dims);
    // Shrink the placement box by the ruin margin so a site's footprint never
    // clips the perimeter wall.
    let min_x = bounds.min_x + RUIN_BOUNDS_MARGIN_M;
    let max_x = bounds.max_x - RUIN_BOUNDS_MARGIN_M;
    let min_z = bounds.min_z + RUIN_BOUNDS_MARGIN_M;
    let max_z = bounds.max_z - RUIN_BOUNDS_MARGIN_M;
    // Degenerate on a tiny world: no room for a ruin.
    if max_x <= min_x || max_z <= min_z {
        return Vec::new();
    }

    // The exclusion ring is measured against the same half-extent the
    // meteorite ring uses (the playable half-extent, i.e. `max_x` of the full
    // bounds), so the two POI gates read consistently.
    let playable_radius = bounds.max_x.max(1.0);
    let exclusion = playable_radius * RUIN_SPAWN_EXCLUSION_RADIUS_FRACTION;
    let exclusion_sq = exclusion * exclusion;
    let spacing_sq = RUIN_MIN_SPACING_M * RUIN_MIN_SPACING_M;

    // A dedicated deterministic stream salted off the seed. Not `ChunkRng`
    // (that is keyed to a chunk); this is a whole-world stream, so it is a bare
    // splitmix64 walk, the same idiom the regrow scheduler uses.
    let mut state = splitmix64(world_seed ^ 0x2011_C0DE_2011_C0DE);
    let mut next = || {
        state = splitmix64(state);
        // Top 24 bits -> [0, 1), matching `ChunkRng::next_unit`.
        let bits = (state >> 40) as u32 & ((1 << 24) - 1);
        bits as f32 / (1u32 << 24) as f32
    };

    let mut sites: Vec<RuinSite> = Vec::new();
    for _ in 0..RUIN_SCATTER_CANDIDATES {
        let x = min_x + (max_x - min_x) * next();
        let z = min_z + (max_z - min_z) * next();
        // Draw the prefab and orientation unconditionally so a rejected
        // candidate still advances the stream by the same amount, keeping the
        // sequence a pure function of the seed regardless of how many
        // candidates pass.
        let prefab_pick = (next() * RuinPrefab::ALL.len() as f32) as usize;
        let prefab = RuinPrefab::ALL[prefab_pick.min(RuinPrefab::ALL.len() - 1)];
        let quarter_turns = ((next() * 4.0) as u8).min(3);

        // Outside the centre ring?
        if x * x + z * z < exclusion_sq {
            continue;
        }
        // Far enough from every accepted site?
        if sites.iter().any(|s| {
            let dx = x - s.x;
            let dz = z - s.z;
            dx * dx + dz * dz < spacing_sq
        }) {
            continue;
        }
        sites.push(RuinSite {
            prefab,
            x,
            z,
            quarter_turns,
        });
    }
    sites
}

// ---------------------------------------------------------------------------
// Prefab collider data. Coordinates are prefab-local metres (+X east, +Z
// south, +Y up), matched by hand to the authored shell geometry in
// `art/ruins/build_ruins.py`; edit the two together. Wall boxes stand on the
// plinth top (base y = FLOOR_TOP_M) at the average surviving height of their
// charred planks. Door / cart openings are real gaps between boxes.
// ---------------------------------------------------------------------------

/// Wall collider thickness (half extent). The authored plank walls are a
/// touch thinner; the collider rounds up so a wall never reads passable.
const WT: f32 = 0.15;
/// Plinth half height; the slab spans ground to `FLOOR_TOP_M`.
const PH: f32 = FLOOR_TOP_M / 2.0;

/// Burnt cottage: one 6.4 x 4.9 m plinth. Back (north, -Z) wall mostly
/// standing, west wall partial, east wall a low stub, front wall split by the
/// door gap. Sloped ("slope"-profile) walls carry TWO collider boxes, a tall
/// half and a low half, so the collider tracks the visual burn line instead
/// of blocking an invisible full-height plane over knee-high planks. The
/// slope always descends toward the wall's +axis end, matching the build
/// script's deterministic envelope.
const COTTAGE_PLINTHS: &[RuinBox] = &[RuinBox::new((0.0, PH, 0.0), (3.2, PH, 2.45))];
const COTTAGE_WALLS: &[RuinBox] = &[
    // Back wall, tallest survivor.
    RuinBox::new((0.0, FLOOR_TOP_M + 1.1, -2.1), (2.85, 1.1, WT)),
    // West wall, sloping down toward +Z: tall half, then low half.
    RuinBox::new((-2.85, FLOOR_TOP_M + 0.9, -0.975), (WT, 0.9, 0.975)),
    RuinBox::new((-2.85, FLOOR_TOP_M + 0.585, 0.975), (WT, 0.585, 0.975)),
    // East wall, burnt to a knee-high stub.
    RuinBox::new((2.85, FLOOR_TOP_M + 0.4, 0.0), (WT, 0.4, 1.95)),
    // Front wall, left of the door gap (gap spans x 0.0..1.2, a touch wider
    // than the standard 1.1 m doorway so an angled approach never snags).
    RuinBox::new((-1.425, FLOOR_TOP_M + 0.5, 2.1), (1.425, 0.5, WT)),
    // Front wall, right of the door gap, sloping down toward +X.
    RuinBox::new((1.6125, FLOOR_TOP_M + 0.7, 2.1), (0.4125, 0.7, WT)),
    RuinBox::new((2.4375, FLOOR_TOP_M + 0.455, 2.1), (0.4125, 0.455, WT)),
    // Stone chimney stub standing over the back wall's east end.
    RuinBox::new((1.9, FLOOR_TOP_M + 1.3, -2.1), (0.28, 1.3, 0.28)),
];

/// Burnt farmhouse: L-shape, a 7.5 x 5.5 m main room plus a 3 x 3.5 m annex
/// off the south-east corner. The north wall still carries most of its gable.
const FARMHOUSE_PLINTHS: &[RuinBox] = &[
    RuinBox::new((-0.75, PH, 0.0), (3.75, PH, 2.75)),
    RuinBox::new((4.5, PH, 1.0), (1.5, PH, 1.75)),
];
const FARMHOUSE_WALLS: &[RuinBox] = &[
    // Main north wall, the tall survivor.
    RuinBox::new((-0.75, FLOOR_TOP_M + 1.15, -2.55), (3.55, 1.15, WT)),
    // Main west wall, sloping down toward +Z: tall half, then low half.
    RuinBox::new((-4.3, FLOOR_TOP_M + 0.8, -1.2), (WT, 0.8, 1.2)),
    RuinBox::new((-4.3, FLOOR_TOP_M + 0.52, 1.2), (WT, 0.52, 1.2)),
    // Main south wall, left of the door gap (gap spans x -1.65..-0.45, a
    // touch wider than the standard 1.1 m doorway).
    RuinBox::new((-2.975, FLOOR_TOP_M + 0.45, 2.55), (1.325, 0.45, WT)),
    // Main south wall, right of the door gap, sloping down toward +X.
    RuinBox::new((0.3625, FLOOR_TOP_M + 0.6, 2.55), (0.8125, 0.6, WT)),
    RuinBox::new((1.9875, FLOOR_TOP_M + 0.39, 2.55), (0.8125, 0.39, WT)),
    // Annex east wall, sloping down toward +Z.
    RuinBox::new((5.8, FLOOR_TOP_M + 0.7, 0.225), (WT, 0.7, 0.775)),
    RuinBox::new((5.8, FLOOR_TOP_M + 0.455, 1.775), (WT, 0.455, 0.775)),
    // Annex south wall.
    RuinBox::new((4.4, FLOOR_TOP_M + 0.5, 2.55), (1.4, 0.5, WT)),
    // Annex north stub.
    RuinBox::new((4.4, FLOOR_TOP_M + 0.3, -0.55), (1.4, 0.3, WT)),
    // Stone chimney stub standing in the west wall.
    RuinBox::new((-4.3, FLOOR_TOP_M + 1.5, -0.9), (0.28, 1.5, 0.28)),
];

/// Burnt shed: a 3.9 x 3.3 m three-sided outbuilding, open across the front
/// (south) side.
const SHED_PLINTHS: &[RuinBox] = &[RuinBox::new((0.0, PH, 0.0), (1.95, PH, 1.65))];
const SHED_WALLS: &[RuinBox] = &[
    // Back wall, sloping down toward +X: tall half, then low half.
    RuinBox::new((-0.85, FLOOR_TOP_M + 0.8, -1.4), (0.85, 0.8, WT)),
    RuinBox::new((0.85, FLOOR_TOP_M + 0.52, -1.4), (0.85, 0.52, WT)),
    // West wall, sloping down toward +Z.
    RuinBox::new((-1.7, FLOOR_TOP_M + 0.6, -0.675), (WT, 0.6, 0.675)),
    RuinBox::new((-1.7, FLOOR_TOP_M + 0.39, 0.675), (WT, 0.39, 0.675)),
    // East wall, lower.
    RuinBox::new((1.7, FLOOR_TOP_M + 0.35, 0.0), (WT, 0.35, 1.35)),
];

/// Burnt barn: an 8.4 x 5.9 m plinth, both gable ends still standing tall,
/// long walls burnt low, a cart opening in the north side (gap spans
/// x -1.2..1.2).
const BARN_PLINTHS: &[RuinBox] = &[RuinBox::new((0.0, PH, 0.0), (4.2, PH, 2.95))];
const BARN_WALLS: &[RuinBox] = &[
    // West gable end, the tall one: low shoulder, full-height peak third,
    // low shoulder, tracking the triangular gable envelope.
    RuinBox::new((-3.9, FLOOR_TOP_M + 0.91, -1.7333), (WT, 0.91, 0.8667)),
    RuinBox::new((-3.9, FLOOR_TOP_M + 1.4, 0.0), (WT, 1.4, 0.8667)),
    RuinBox::new((-3.9, FLOOR_TOP_M + 0.91, 1.7333), (WT, 0.91, 0.8667)),
    // East gable end, same shape, lower.
    RuinBox::new((3.9, FLOOR_TOP_M + 0.65, -1.7333), (WT, 0.65, 0.8667)),
    RuinBox::new((3.9, FLOOR_TOP_M + 1.0, 0.0), (WT, 1.0, 0.8667)),
    RuinBox::new((3.9, FLOOR_TOP_M + 0.65, 1.7333), (WT, 0.65, 0.8667)),
    // North wall, west of the cart opening.
    RuinBox::new((-2.55, FLOOR_TOP_M + 0.5, -2.6), (1.35, 0.5, WT)),
    // North wall, east of the cart opening.
    RuinBox::new((2.55, FLOOR_TOP_M + 0.5, -2.6), (1.35, 0.5, WT)),
    // South wall, one long low run.
    RuinBox::new((0.0, FLOOR_TOP_M + 0.35, 2.6), (3.75, 0.35, WT)),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn medium_dims() -> ChunkDims {
        ChunkDims::new(31)
    }

    #[test]
    fn layout_is_deterministic_per_seed() {
        let a = ruin_layout(0xA11CE, medium_dims());
        let b = ruin_layout(0xA11CE, medium_dims());
        assert_eq!(a.len(), b.len());
        for (sa, sb) in a.iter().zip(b.iter()) {
            assert_eq!(sa.prefab, sb.prefab);
            assert_eq!(sa.x, sb.x);
            assert_eq!(sa.z, sb.z);
            assert_eq!(sa.quarter_turns, sb.quarter_turns);
        }
    }

    #[test]
    fn different_seeds_produce_different_layouts() {
        let a = ruin_layout(1, medium_dims());
        let b = ruin_layout(2, medium_dims());
        // Extremely unlikely to match on both count and every position.
        let identical = a.len() == b.len()
            && a.iter()
                .zip(b.iter())
                .all(|(sa, sb)| sa.x == sb.x && sa.z == sb.z);
        assert!(!identical, "distinct seeds should scatter differently");
    }

    #[test]
    fn a_medium_world_scatters_a_handful_of_ruins() {
        // Not asserting an exact count (it depends on the seed), just that a
        // real world lands a handful of landmark ruins, never zero and never a
        // saturated field.
        let mut total = 0usize;
        for seed in [1u64, 7, 42, 1234, 99999] {
            let sites = ruin_layout(seed, medium_dims());
            assert!(
                (2..=30).contains(&sites.len()),
                "seed {seed}: expected a handful of ruins, got {}",
                sites.len()
            );
            total += sites.len();
        }
        assert!(total > 0);
    }

    #[test]
    fn a_large_world_holds_more_ruins_than_a_small_one() {
        // The scatter fills more of a bigger world, so on average large > small.
        let mut small_total = 0usize;
        let mut large_total = 0usize;
        for seed in 0..12u64 {
            small_total += ruin_layout(seed, ChunkDims::new(15)).len();
            large_total += ruin_layout(seed, ChunkDims::new(63)).len();
        }
        assert!(
            large_total > small_total,
            "a large world should hold more ruins overall (small {small_total} vs large {large_total})"
        );
    }

    #[test]
    fn sites_respect_min_spacing() {
        for seed in 0..40u64 {
            let sites = ruin_layout(seed, medium_dims());
            for i in 0..sites.len() {
                for j in (i + 1)..sites.len() {
                    let dx = sites[i].x - sites[j].x;
                    let dz = sites[i].z - sites[j].z;
                    let dist = (dx * dx + dz * dz).sqrt();
                    assert!(
                        dist + 1e-2 >= RUIN_MIN_SPACING_M,
                        "seed {seed}: ruins {i}/{j} too close: {dist} < {RUIN_MIN_SPACING_M}"
                    );
                }
            }
        }
    }

    #[test]
    fn sites_stay_outside_the_center_ring_and_inside_bounds() {
        let dims = medium_dims();
        let bounds = PlayableBounds::from_dims(dims);
        let exclusion = bounds.max_x.max(1.0) * RUIN_SPAWN_EXCLUSION_RADIUS_FRACTION;
        for seed in 0..40u64 {
            for site in ruin_layout(seed, dims) {
                let dist = (site.x * site.x + site.z * site.z).sqrt();
                assert!(
                    dist + 1e-2 >= exclusion,
                    "seed {seed}: ruin at ({}, {}) is inside the exclusion ring ({dist} < {exclusion})",
                    site.x,
                    site.z
                );
                assert!(
                    site.x >= bounds.min_x + RUIN_BOUNDS_MARGIN_M - 1e-2
                        && site.x <= bounds.max_x - RUIN_BOUNDS_MARGIN_M + 1e-2
                        && site.z >= bounds.min_z + RUIN_BOUNDS_MARGIN_M - 1e-2
                        && site.z <= bounds.max_z - RUIN_BOUNDS_MARGIN_M + 1e-2,
                    "seed {seed}: ruin at ({}, {}) escaped the margined bounds",
                    site.x,
                    site.z
                );
            }
        }
    }

    #[test]
    fn orientations_are_quarter_turns_and_varied() {
        // Every site's orientation is a legal quarter turn, and across seeds
        // the draw actually uses the whole range (variation, not a constant).
        let mut seen = [false; 4];
        for seed in 0..20u64 {
            for site in ruin_layout(seed, medium_dims()) {
                assert!(site.quarter_turns < 4, "quarter turns must be 0..=3");
                seen[site.quarter_turns as usize] = true;
            }
        }
        assert!(
            seen.iter().all(|s| *s),
            "all four orientations should appear across seeds: {seen:?}"
        );
    }

    #[test]
    fn every_prefab_has_between_one_and_three_cache_points() {
        for prefab in RuinPrefab::ALL {
            let n = prefab.cache_points().len();
            assert!(
                (1..=3).contains(&n),
                "{prefab:?} must carry 1..=3 chests, has {n}"
            );
        }
    }

    #[test]
    fn cache_points_sit_inside_the_footprint() {
        // Each prefab's chest points must be within the footprint radius of the
        // site centre, or the node-rejection footprint wouldn't protect them.
        for prefab in RuinPrefab::ALL {
            let radius = prefab.footprint_radius_m();
            for &(lx, lz) in prefab.cache_points() {
                let dist = (lx * lx + lz * lz).sqrt();
                assert!(
                    dist <= radius,
                    "{prefab:?} chest at ({lx}, {lz}) is {dist} from centre, outside footprint {radius}"
                );
            }
        }
    }

    #[test]
    fn collider_boxes_sit_inside_the_footprint() {
        // The footprint circle must enclose every collider box corner, or the
        // node-rejection gate could let a node spawn inside a wall.
        for prefab in RuinPrefab::ALL {
            let radius = prefab.footprint_radius_m();
            for b in prefab.plinth_boxes().iter().chain(prefab.wall_boxes()) {
                let (cx, _, cz) = b.center;
                let (hx, _, hz) = b.half;
                let corner = ((cx.abs() + hx).powi(2) + (cz.abs() + hz).powi(2)).sqrt();
                assert!(
                    corner <= radius,
                    "{prefab:?} box at ({cx}, {cz}) reaches {corner}, outside footprint {radius}"
                );
            }
        }
    }

    #[test]
    fn cache_points_stand_on_a_plinth() {
        // `RuinSite::cache_points` spawns every chest at the plinth TOP
        // (`FLOOR_TOP_M`), which is only correct if each local chest point
        // actually lies over a plinth slab's footprint. Pin that data
        // invariant per prefab so a future prefab edit can't leave a chest
        // floating in the air or embedded in the ground.
        for prefab in RuinPrefab::ALL {
            for &(cx, cz) in prefab.cache_points() {
                let covered = prefab.plinth_boxes().iter().any(|b| {
                    (b.center.0 - cx).abs() <= b.half.0 && (b.center.2 - cz).abs() <= b.half.2
                });
                assert!(
                    covered,
                    "{prefab:?} chest at ({cx}, {cz}) is not over any plinth slab"
                );
            }
        }
        // And the world-space spawn height is exactly the plinth top.
        let site = RuinSite {
            prefab: RuinPrefab::BurntCottage,
            x: 100.0,
            z: -40.0,
            quarter_turns: 3,
        };
        for point in site.cache_points() {
            assert_eq!(point.y, FLOOR_TOP_M);
        }
    }

    #[test]
    fn every_prefab_has_plinths_and_walls() {
        for prefab in RuinPrefab::ALL {
            assert!(
                !prefab.plinth_boxes().is_empty(),
                "{prefab:?} has no floor plinth"
            );
            assert!(
                prefab.wall_boxes().len() >= 3,
                "{prefab:?} should keep at least three wall stubs standing"
            );
        }
    }

    #[test]
    fn wall_boxes_stand_on_the_plinth_top() {
        // Every wall box's base must sit exactly at the plinth top, so the
        // authored shell (whose planks rise off the plinth) and the collider
        // agree vertically.
        for prefab in RuinPrefab::ALL {
            for b in prefab.wall_boxes() {
                let base = b.center.1 - b.half.1;
                assert!(
                    (base - FLOOR_TOP_M).abs() < 1e-5,
                    "{prefab:?} wall box base {base} should sit at the plinth top {FLOOR_TOP_M}"
                );
            }
        }
    }

    #[test]
    fn static_blocks_rotate_exactly_with_the_site() {
        // A quarter-turned site's blocks are the same boxes with X/Z swapped
        // (rotated about the site centre), never a loose enclosure.
        let flat = RuinSite {
            prefab: RuinPrefab::BurntBarn,
            x: 0.0,
            z: 0.0,
            quarter_turns: 0,
        };
        let turned = RuinSite {
            quarter_turns: 1,
            ..flat
        };
        let flat_blocks = flat.static_blocks();
        let turned_blocks = turned.static_blocks();
        assert_eq!(flat_blocks.len(), turned_blocks.len());
        for (a, b) in flat_blocks.iter().zip(turned_blocks.iter()) {
            // One quarter turn under Bevy's `from_rotation_y(FRAC_PI_2)`
            // (the quaternion the client renders the shell with, and the
            // same mapping as `building.rs - rotate_offset` step 1):
            // (x, z) -> (z, -x). Centres rotate, half extents swap. If this
            // ever disagrees with the rendered shell again, the collider
            // layout stands 180 degrees against the visuals on q=1/q=3
            // sites (an invisible wall in the visible doorway).
            assert!((b.center.x - a.center.z).abs() < 1e-5);
            assert!((b.center.z - -a.center.x).abs() < 1e-5);
            assert!((b.half_extents.x - a.half_extents.z).abs() < 1e-5);
            assert!((b.half_extents.z - a.half_extents.x).abs() < 1e-5);
            assert_eq!(a.center.y, b.center.y);
            assert_eq!(a.half_extents.y, b.half_extents.y);
        }
    }

    #[test]
    fn static_blocks_are_produced_for_every_site() {
        let sites = ruin_layout(42, medium_dims());
        for site in &sites {
            let blocks = site.static_blocks();
            assert!(
                !blocks.is_empty(),
                "site {:?} produced no collision blocks",
                site.prefab
            );
        }
    }

    #[test]
    fn map_glyphs_and_worldgen_share_one_layout() {
        // The client map draws a glyph per site from `ruin_layout`, and the
        // server worldgen spawns chests / static blocks from the SAME call.
        // This pins that they are byte-identical for a given seed, so a glyph
        // can never sit where there is no ruin (singleplayer == multiplayer).
        for seed in [TEST_WORLD_SEED_FIXTURE, 1, 55, 777] {
            let dims = medium_dims();
            let map_side = ruin_layout(seed, dims);
            let worldgen_side = ruin_layout(seed, dims);
            assert_eq!(map_side.len(), worldgen_side.len());
            for (m, w) in map_side.iter().zip(worldgen_side.iter()) {
                assert_eq!((m.x, m.z, m.prefab), (w.x, w.z, w.prefab));
            }
        }
    }

    const TEST_WORLD_SEED_FIXTURE: u64 = 0x7E57_5EED_5EED_5EED;

    #[test]
    fn footprints_reject_points_inside_and_accept_outside() {
        let sites = ruin_layout(7, medium_dims());
        let footprints = ruin_footprints(&sites);
        if let Some(site) = sites.first() {
            // A point at the exact centre is inside.
            assert!(point_in_any_footprint(&footprints, site.x, site.z));
            // A point one full spacing away (past any single footprint) is out.
            assert!(!point_in_any_footprint(
                &footprints,
                site.x + RUIN_MIN_SPACING_M,
                site.z
            ));
        }
    }
}
