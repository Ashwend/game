//! Ruin structures: deterministic point-of-interest worldgen.
//!
//! A ruin is a small hand-authored prefab (collapsed shrine, broken forge,
//! watchtower stub) kitbashed from stone-tier building-piece meshes plus a few
//! new ruin props, scattered across the world at generation time and carrying
//! one to three refilling loot caches. Everything in this module is a **pure
//! function of `(world_seed, dims)`**, exactly like the resource-node spawn
//! pipeline, so the server (worldgen + cache spawning) and the client (map
//! glyphs) compute the identical layout with no wire traffic and no divergence
//! risk (singleplayer == multiplayer).
//!
//! The load-bearing entry point is [`ruin_layout`]: same seed, same sites;
//! different seeds, different sites. From a layout you can derive:
//!
//! - [`ruin_footprints`] for the node-rejection gate in the chunk generator
//!   (nodes must not spawn inside a ruin),
//! - [`RuinSite::static_blocks`] for the collision/LoS geometry registered as
//!   world blocks (the perimeter-wall precedent),
//! - [`RuinSite::cache_points`] for the world-space cache spawn positions, and
//! - [`RuinSite::render_elements`] for the client's mesh spawns.
//!
//! Placement (in [`ruin_layout`]) is Poisson-style rejection sampling seeded
//! from the world seed: candidates are drawn across the playable interior,
//! rejected if inside the centre exclusion ring, outside the bounds margin, or
//! within `RUIN_MIN_SPACING_M` of an already-accepted site.

use crate::{
    building::{BuildingPiece, BuildingTier, FOUNDATION_SIZE_M, building_collider_blocks},
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

/// The three hand-authored ruin prefabs. Appended-only; the discriminant is
/// never persisted (a ruin layout is recomputed from the seed on every load),
/// so ordering is free to change, but keep it stable for readable tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuinPrefab {
    /// A ring of stone foundations with three broken wall stubs and a fallen
    /// arch over the old entrance, a single cache in the sunken centre.
    CollapsedShrine,
    /// A stone platform with a half-standing forge wall, a leaning pillar, and
    /// two caches where the smiths' stores would have been.
    BrokenForge,
    /// A 2x2 foundation ring with three broken wall segments at varying heights
    /// and a broken pillar, one cache tucked against the tallest wall.
    WatchtowerStub,
}

impl RuinPrefab {
    /// Every prefab, in a stable order. The scatter picks from this by index.
    pub const ALL: [Self; 3] = [
        Self::CollapsedShrine,
        Self::BrokenForge,
        Self::WatchtowerStub,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::CollapsedShrine => "Collapsed Shrine",
            Self::BrokenForge => "Broken Forge",
            Self::WatchtowerStub => "Watchtower Stub",
        }
    }

    /// Prefab-local element list. Coordinates are in metres relative to the
    /// site centre (`+X` east, `+Z` south, `+Y` up), pre-rotation. The site's
    /// own yaw rotates the whole set into the world.
    pub fn elements(self) -> &'static [RuinElement] {
        match self {
            Self::CollapsedShrine => COLLAPSED_SHRINE,
            Self::BrokenForge => BROKEN_FORGE,
            Self::WatchtowerStub => WATCHTOWER_STUB,
        }
    }

    /// Prefab-local cache spawn points (1 to 3 per prefab). Each is a local
    /// XZ offset from the site centre; every point lies over a foundation
    /// element (pinned by test), and [`RuinSite::cache_points`] spawns the
    /// cache at the foundation TOP so it sits proud on the platform.
    pub fn cache_points(self) -> &'static [(f32, f32)] {
        match self {
            Self::CollapsedShrine => &[(0.0, 0.0)],
            Self::BrokenForge => &[(-2.2, 1.6), (2.4, -1.2)],
            Self::WatchtowerStub => &[(1.1, 1.1)],
        }
    }

    /// Radius, in metres, of the site's circular footprint. Node spawns inside
    /// this circle are rejected, so it is sized to enclose the whole prefab
    /// plus a little clearance. A generous single circle is far cheaper to test
    /// per node than a per-element polygon and reads the same in practice.
    pub const fn footprint_radius_m(self) -> f32 {
        match self {
            // Both the shrine and forge sprawl a bit wider than the tower stub.
            Self::CollapsedShrine => 7.5,
            Self::BrokenForge => 8.0,
            Self::WatchtowerStub => 6.0,
        }
    }
}

/// One placed piece of a prefab. Either a reused stone building-piece mesh or
/// one of the new ruin props. `local` is the piece base centre in prefab-local
/// space; `yaw` is a local rotation added to the site yaw; `height_scale`
/// truncates wall-like pieces so a ruin reads as broken (1.0 = full height).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuinElement {
    pub kind: RuinElementKind,
    pub local_x: f32,
    pub local_z: f32,
    pub local_yaw: f32,
    /// Fraction of full height the element keeps. Applies to wall-like building
    /// pieces (a "broken wall" is a short wall) and to tall props. `1.0` for a
    /// full-height or naturally-short element.
    pub height_scale: f32,
}

impl RuinElement {
    const fn building(piece: BuildingPiece, x: f32, z: f32, yaw: f32, height: f32) -> Self {
        Self {
            kind: RuinElementKind::Building(piece),
            local_x: x,
            local_z: z,
            local_yaw: yaw,
            height_scale: height,
        }
    }

    const fn prop(prop: RuinProp, x: f32, z: f32, yaw: f32) -> Self {
        Self {
            kind: RuinElementKind::Prop(prop),
            local_x: x,
            local_z: z,
            local_yaw: yaw,
            height_scale: 1.0,
        }
    }
}

/// What an element renders and collides as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuinElementKind {
    /// A stone-tier building piece reused as static ruin masonry. Renders with
    /// the client's existing building mesh for `(piece, Stone)`; collides via
    /// [`building_collider_blocks`].
    Building(BuildingPiece),
    /// One of the new authored ruin props.
    Prop(RuinProp),
}

/// The new authored ruin props (cel-shaded world glbs under `assets/ruins/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuinProp {
    /// Tapered fluted pillar stump with a fractured top.
    BrokenPillar,
    /// Two pillar stumps carrying a collapsed lintel lying at an angle.
    FallenArch,
}

impl RuinProp {
    pub const fn asset_stem(self) -> &'static str {
        match self {
            Self::BrokenPillar => "broken_pillar",
            Self::FallenArch => "fallen_arch",
        }
    }

    /// Half-extents of the prop's collision box, in metres, before the
    /// element's `height_scale` and the site yaw are applied. Kept simple: a
    /// single upright AABB is enough for movement collision and line-of-sight
    /// against a stumpy prop.
    const fn collider_half_extents(self) -> (f32, f32, f32) {
        match self {
            // A stubby pillar: ~0.9 m tall (half 0.45), ~0.35 m radius.
            Self::BrokenPillar => (0.35, 0.45, 0.35),
            // A wide, low arch span: ~2.6 m across, ~1.2 m tall.
            Self::FallenArch => (1.3, 0.6, 0.6),
        }
    }
}

/// One scattered ruin: which prefab, where (world XZ), and its world yaw. The
/// ground height is `y = 0` (the world floor is flat); this stays a field so a
/// future heightmap can snap it without changing the shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuinSite {
    pub prefab: RuinPrefab,
    pub x: f32,
    pub z: f32,
    pub yaw: f32,
}

impl RuinSite {
    /// World-space centre of the site as a [`Vec3Net`] at ground level.
    pub fn center(self) -> Vec3Net {
        Vec3Net::new(self.x, 0.0, self.z)
    }

    /// Rotate a prefab-local XZ offset by the site yaw into world space.
    fn local_to_world(self, local_x: f32, local_z: f32) -> (f32, f32) {
        let (sin, cos) = self.yaw.sin_cos();
        (
            self.x + local_x * cos - local_z * sin,
            self.z + local_x * sin + local_z * cos,
        )
    }

    /// World-space cache spawn positions. Every prefab places its cache points
    /// on the foundation slab, so a cache's base sits at the foundation TOP
    /// (`FOUNDATION_HEIGHT_M`), proud on the platform rather than half-embedded
    /// in it. If a future prefab ever puts a cache on bare ground, make the
    /// height per-point prefab data instead of this constant.
    pub fn cache_points(self) -> Vec<Vec3Net> {
        self.prefab
            .cache_points()
            .iter()
            .map(|&(lx, lz)| {
                let (wx, wz) = self.local_to_world(lx, lz);
                Vec3Net::new(wx, crate::building::FOUNDATION_HEIGHT_M, wz)
            })
            .collect()
    }

    /// Static collision/LoS blocks for the whole site, in world space. These
    /// are registered as [`WorldBlock`]s at world build (the perimeter-wall
    /// precedent), so the `BlockGrid` gives collision, projectile LoS, and
    /// melee LoS for free.
    pub fn static_blocks(self) -> Vec<WorldBlock> {
        let mut blocks = Vec::new();
        for element in self.prefab.elements() {
            let (wx, wz) = self.local_to_world(element.local_x, element.local_z);
            let world_yaw = self.yaw + element.local_yaw;
            match element.kind {
                RuinElementKind::Building(piece) => {
                    // Reuse the exact building collider geometry so ruin
                    // masonry collides like a real wall/foundation. Broken
                    // (height-scaled) walls are still full-footprint solid at
                    // the base, which is what a rubble stub should be. Retag
                    // each box as ruin masonry so the scene renderer skips the
                    // plain cuboid and the ruin system draws the real mesh.
                    let position = Vec3Net::new(wx, 0.0, wz);
                    blocks.extend(
                        building_collider_blocks(piece, position, world_yaw)
                            .into_iter()
                            .map(|b| b.with_kind(crate::world::BlockKind::RuinMasonry)),
                    );
                }
                RuinElementKind::Prop(prop) => {
                    let (hx, hy, hz) = prop.collider_half_extents();
                    let hy = hy * element.height_scale;
                    // Rotate the box's half-extents footprint by the yaw. For
                    // an axis-aligned grid we keep the box axis-aligned and use
                    // the larger horizontal half-extent so the collider fully
                    // encloses the rotated prop (a slightly loose but never
                    // pass-through fit, matching the AABB-only pipeline).
                    let horizontal = hx.max(hz);
                    let center = Vec3Net::new(wx, hy, wz);
                    let half = Vec3Net::new(horizontal, hy, horizontal);
                    blocks.push(WorldBlock::ruin(center, half));
                }
            }
        }
        blocks
    }

    /// Per-element render transforms for the client: `(kind, world position,
    /// world yaw, height_scale)`. The client maps each `Building(piece)` to the
    /// stone-tier building mesh and each `Prop` to its glb, applying the height
    /// scale as a Y scale.
    pub fn render_elements(self) -> Vec<RuinRenderElement> {
        self.prefab
            .elements()
            .iter()
            .map(|element| {
                let (wx, wz) = self.local_to_world(element.local_x, element.local_z);
                RuinRenderElement {
                    kind: element.kind,
                    position: Vec3Net::new(wx, 0.0, wz),
                    yaw: self.yaw + element.local_yaw,
                    height_scale: element.height_scale,
                }
            })
            .collect()
    }
}

/// A single element resolved to world space for rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuinRenderElement {
    pub kind: RuinElementKind,
    pub position: Vec3Net,
    pub yaw: f32,
    pub height_scale: f32,
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

/// **The** shared, deterministic ruin scatter. A pure function of
/// `(world_seed, dims)`: the server calls it at world build (for static blocks,
/// cache spawns, and the node-rejection footprints) and the client calls it for
/// the map glyphs, so both agree with zero coordination.
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
/// Each accepted site draws its prefab and yaw from the same seeded stream, so
/// the whole layout round-trips identically.
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
        // Draw the prefab and yaw unconditionally so a rejected candidate still
        // advances the stream by the same amount, keeping the sequence a pure
        // function of the seed regardless of how many candidates pass.
        let prefab_pick = (next() * RuinPrefab::ALL.len() as f32) as usize;
        let prefab = RuinPrefab::ALL[prefab_pick.min(RuinPrefab::ALL.len() - 1)];
        let yaw = (next() - 0.5) * std::f32::consts::TAU;

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
        sites.push(RuinSite { prefab, x, z, yaw });
    }
    sites
}

// ---------------------------------------------------------------------------
// Prefab data. Coordinates are prefab-local metres; the site yaw rotates the
// whole set. `FOUNDATION_SIZE_M` (3 m) is the grid the building pieces snap to,
// so foundations sit on a 3 m lattice and walls span a foundation edge.
// ---------------------------------------------------------------------------

const F: f32 = FOUNDATION_SIZE_M; // 3.0 m grid step, for readable offsets.
const HALF_TURN: f32 = std::f32::consts::PI;
const QUARTER_TURN: f32 = std::f32::consts::FRAC_PI_2;

/// Collapsed shrine: a small ring of four stone foundations with a fallen arch
/// over the entrance and two broken wall stubs. The single cache sits in the
/// centre. 4 foundations + 2 broken walls + 1 fallen arch + 1 broken pillar.
const COLLAPSED_SHRINE: &[RuinElement] = &[
    // A 2x2 foundation slab.
    RuinElement::building(BuildingPiece::Foundation, -F * 0.5, -F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, F * 0.5, -F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, -F * 0.5, F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, F * 0.5, F * 0.5, 0.0, 1.0),
    // Two broken back walls at differing heights.
    RuinElement::building(BuildingPiece::Wall, -F * 0.5, -F, 0.0, 0.55),
    RuinElement::building(BuildingPiece::Wall, F * 0.5, -F, 0.0, 0.35),
    // A fallen arch across the front entrance.
    RuinElement::prop(RuinProp::FallenArch, 0.0, F, QUARTER_TURN),
    // A toppled pillar off to one side.
    RuinElement::prop(RuinProp::BrokenPillar, -F, F * 0.5, 0.0),
];

/// Broken forge: a wide stone platform with a half-standing forge wall, a
/// leaning pillar, and two caches. 6 foundations + 3 broken walls + 1 broken
/// pillar.
const BROKEN_FORGE: &[RuinElement] = &[
    // A 3x2 platform.
    RuinElement::building(BuildingPiece::Foundation, -F, -F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, 0.0, -F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, F, -F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, -F, F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, 0.0, F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, F, F * 0.5, 0.0, 1.0),
    // A half-standing forge wall across the back, plus two shorter stubs.
    RuinElement::building(BuildingPiece::Wall, 0.0, -F, 0.0, 0.7),
    RuinElement::building(BuildingPiece::Wall, -F, -F, 0.0, 0.4),
    RuinElement::building(BuildingPiece::Wall, -F * 1.5, 0.0, QUARTER_TURN, 0.5),
    // A leaning pillar by the smith's corner.
    RuinElement::prop(RuinProp::BrokenPillar, F * 1.2, F, 0.0),
];

/// Watchtower stub: a 2x2 foundation ring with three broken wall segments at
/// varying heights and a broken pillar. One cache. 4 foundations + 3 broken
/// walls + 1 broken pillar.
const WATCHTOWER_STUB: &[RuinElement] = &[
    // A 2x2 base ring.
    RuinElement::building(BuildingPiece::Foundation, -F * 0.5, -F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, F * 0.5, -F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, -F * 0.5, F * 0.5, 0.0, 1.0),
    RuinElement::building(BuildingPiece::Foundation, F * 0.5, F * 0.5, 0.0, 1.0),
    // Three broken wall segments at varying heights around the ring.
    RuinElement::building(BuildingPiece::Wall, -F * 0.5, -F, 0.0, 0.8),
    RuinElement::building(BuildingPiece::Wall, F * 0.5, -F, 0.0, 0.5),
    RuinElement::building(BuildingPiece::Wall, -F, -F * 0.5, QUARTER_TURN, 0.3),
    // A broken pillar at the far corner.
    RuinElement::prop(RuinProp::BrokenPillar, F * 0.5, F, HALF_TURN),
];

/// The stone tier all ruin masonry renders at (weathered stone building
/// pieces). Exposed so the client mesh lookup uses the same tier as the
/// collider geometry.
pub const RUIN_MASONRY_TIER: BuildingTier = BuildingTier::Stone;

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
            assert_eq!(sa.yaw, sb.yaw);
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
    fn every_prefab_has_between_one_and_three_cache_points() {
        for prefab in RuinPrefab::ALL {
            let n = prefab.cache_points().len();
            assert!(
                (1..=3).contains(&n),
                "{prefab:?} must carry 1..=3 caches, has {n}"
            );
        }
    }

    #[test]
    fn cache_points_sit_inside_the_footprint() {
        // Each prefab's cache points must be within the footprint radius of the
        // site centre, or the node-rejection footprint wouldn't protect them.
        for prefab in RuinPrefab::ALL {
            let radius = prefab.footprint_radius_m();
            for &(lx, lz) in prefab.cache_points() {
                let dist = (lx * lx + lz * lz).sqrt();
                assert!(
                    dist <= radius,
                    "{prefab:?} cache at ({lx}, {lz}) is {dist} from centre, outside footprint {radius}"
                );
            }
        }
    }

    #[test]
    fn cache_points_stand_on_a_foundation_element() {
        // `RuinSite::cache_points` spawns every cache at the foundation TOP
        // (`FOUNDATION_HEIGHT_M`), which is only correct if each local cache
        // point actually lies over a Foundation element's 3 m footprint. Pin
        // that data invariant per prefab so a future prefab edit can't leave a
        // cache floating in the air or embedded in the ground.
        let half = FOUNDATION_SIZE_M / 2.0;
        for prefab in RuinPrefab::ALL {
            for &(cx, cz) in prefab.cache_points() {
                let covered = prefab.elements().iter().any(|e| {
                    matches!(e.kind, RuinElementKind::Building(BuildingPiece::Foundation))
                        && (e.local_x - cx).abs() <= half
                        && (e.local_z - cz).abs() <= half
                });
                assert!(
                    covered,
                    "{prefab:?} cache at ({cx}, {cz}) is not over any foundation element"
                );
            }
        }
        // And the world-space spawn height is exactly the foundation top.
        let site = RuinSite {
            prefab: RuinPrefab::CollapsedShrine,
            x: 100.0,
            z: -40.0,
            yaw: 0.7,
        };
        for point in site.cache_points() {
            assert_eq!(point.y, crate::building::FOUNDATION_HEIGHT_M);
        }
    }

    #[test]
    fn every_prefab_has_elements_and_at_least_one_prop() {
        for prefab in RuinPrefab::ALL {
            let elements = prefab.elements();
            assert!(!elements.is_empty(), "{prefab:?} has no elements");
            let props = elements
                .iter()
                .filter(|e| matches!(e.kind, RuinElementKind::Prop(_)))
                .count();
            assert!(props >= 1, "{prefab:?} should use at least one ruin prop");
            let buildings = elements
                .iter()
                .filter(|e| matches!(e.kind, RuinElementKind::Building(_)))
                .count();
            assert!(
                buildings >= 1,
                "{prefab:?} should reuse at least one building piece"
            );
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
        // server worldgen spawns caches / static blocks from the SAME call.
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
