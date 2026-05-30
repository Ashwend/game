//! Procedural detail grass — a client-only cosmetic ground layer streamed in
//! tiles around the camera, with a custom wind + distance-fade shader.
//!
//! Placement is **seed-free**: which layout + rotation each tile gets is a pure
//! hash of its world-XZ tile coords, deterministic without any server data and
//! identical in singleplayer and multiplayer. Nothing here touches gameplay,
//! collision, the protocol, or the dedicated server.
//!
//! ## Material
//!
//! Grass uses [`GrassMaterial`] — an `ExtendedMaterial<StandardMaterial,
//! GrassWindExtension>` (the project's first custom shader, `assets/shaders/
//! grass.wgsl`). It keeps StandardMaterial's PBR/IBL lighting (so grass is lit
//! like the rest of the scene) and adds:
//! - **Vertex wind sway** — tips ride a world-space wave, weighted by the sway
//!   weight baked into vertex-colour alpha (0 base → 1 tip), so blades bend.
//! - **Fragment radial dither** — whole blades drop out with distance from the
//!   camera (key = a stable per-blade random in `uv.x`), thinning the field into
//!   smooth *rings*. This replaces a hard despawn edge and the old square
//!   per-tile density bands: the fade is now per-blade and radial, GPU-side.
//!
//! ## Two render traps this design avoids
//!
//! 1. **No per-tile mesh assets.** Creating/freeing a mesh per tile as it
//!    streams makes Bevy's `MeshAllocator` repack shared "general slabs",
//!    corrupting other small meshes in them. So we bake a fixed pool of
//!    [`GRASS_LAYOUT_COUNT`] meshes **once** (rebuilt only when density changes)
//!    and only stream entities that reference them.
//! 2. **No `VisibilityRange` on grass.** Bevy rebuilds one global visibility-
//!    range table whenever any `VisibilityRange` entity is added/removed; the
//!    trees use it for LOD, so a grass tile streaming every step would clobber
//!    that table and flicker whole regions of trees. The shader's distance fade
//!    replaces what `VisibilityRange` would have given.

use std::collections::HashMap;

use bevy::{
    asset::RenderAssetUsages,
    light::NotShadowCaster,
    mesh::{Indices, PrimitiveTopology},
    pbr::{ExtendedMaterial, MaterialExtension},
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

use crate::{
    app::state::{ClientRuntime, ClientSettings, GrassDensity},
    world::{WorldBlock, WorldData, fbm, splitmix64},
};

/// Embedded path of the grass shader (see [`crate::app::embedded_asset_path`] —
/// the same `embedded://` scheme, but a `&'static str` because [`ShaderRef`]
/// needs one).
const GRASS_SHADER_PATH: &str = "embedded://shaders/grass.wgsl";

/// Side length of a grass streaming tile / variant mesh, in metres.
const GRASS_TILE_M: f32 = 8.0;

/// Keep grass this far inside the floor edge so tiles never clip the perimeter
/// walls or hang past the ground plane.
const GRASS_EDGE_MARGIN_M: f32 = 3.0;

/// Tile entities spawned per frame. One entity per tile (the mesh is shared and
/// pre-built), so this is cheap — but capped so the first fill on entering a
/// world drains over a few frames instead of one command-buffer spike.
const MAX_GRASS_TILE_SPAWNS_PER_FRAME: usize = 12;

/// Distinct blade layouts to bake. Each tile picks one by hash and rotates it to
/// one of four cardinal angles, giving `4 × COUNT` permutations — plenty to hide
/// tiling across an open field.
const GRASS_LAYOUT_COUNT: usize = 16;

/// Fixed seeds, decoupled from any world seed (placement is seed-free).
const GRASS_CLUMP_SEED: u64 = 0x6A09_E667_F3BC_C909;
const GRASS_LAYOUT_SEED: u64 = 0xBB67_AE85_84CA_A73B;

/// The grass material: StandardMaterial PBR + the wind/dither shader extension.
pub(crate) type GrassMaterial = ExtendedMaterial<StandardMaterial, GrassWindExtension>;

/// Shader extension that adds the wind + distance-fade behaviour. **Deliberately
/// binding-free**: it carries no uniform/texture, only the shader override.
/// `ExtendedMaterial`'s bind-group merge with the bindless `StandardMaterial`
/// drops a `@binding(100)` extension uniform from the pipeline layout on Metal
/// (crash: "binding 100 missing from pipeline layout"), so all shader tuning is
/// compile-time constants in `grass.wgsl` instead — the trade-off being a fixed
/// fade window / draw radius across density tiers (see [`GRASS_RADIUS_M`]).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub(crate) struct GrassWindExtension {}

impl MaterialExtension for GrassWindExtension {
    fn vertex_shader() -> ShaderRef {
        GRASS_SHADER_PATH.into()
    }
    fn fragment_shader() -> ShaderRef {
        GRASS_SHADER_PATH.into()
    }
}

/// Camera-relative radius (m) within which grass tiles are kept loaded. Fixed
/// across density tiers because the shader's fade window (`FADE_START`/`FADE_END`
/// in `grass.wgsl`) is a compile-time constant — see [`GrassWindExtension`]. A
/// hair above the shader's `FADE_END` (45 m) so grass is fully faded before a
/// tile despawns (no pop).
const GRASS_RADIUS_M: f32 = 47.0;

/// Per-tier blade density (blades per square metre before clumping). Density is
/// the only thing the setting changes now; the radius is fixed.
fn blades_per_m2(density: GrassDensity) -> Option<f32> {
    match density {
        GrassDensity::Off => None,
        GrassDensity::Low => Some(6.0),
        GrassDensity::Medium => Some(11.0),
        GrassDensity::High => Some(17.0),
    }
}

/// Marker for a streamed grass-tile entity.
#[derive(Component)]
pub(crate) struct GrassTile;

/// Streaming bookkeeping for the detail grass.
#[derive(Resource, Default)]
pub(crate) struct GrassState {
    /// Loaded tiles by `(tile_x, tile_z)`. `None` marks a tile that's
    /// permanently bare for this world (off the floor or covering a block).
    tiles: HashMap<(i32, i32), Option<Entity>>,
    /// Shared, pre-baked layout meshes (one per layout). Built once per density
    /// and **never freed during play**.
    variants: Vec<Handle<Mesh>>,
    /// Shared grass material, created on first use.
    material: Option<Handle<GrassMaterial>>,
    bound_world_version: u64,
    bound_density: Option<GrassDensity>,
}

/// Stream detail-grass tiles around the camera. The shader handles the distance
/// fade + wind, so the CPU side just spawns/despawns tile entities.
pub(crate) fn stream_grass_system(
    mut commands: Commands,
    settings: Res<ClientSettings>,
    runtime: Res<ClientRuntime>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GrassMaterial>>,
    mut state: ResMut<GrassState>,
) {
    let density = settings.graphics.grass_density;

    // Only stream grass inside a live world; clear the live tiles otherwise.
    let Some(world) = runtime.world.as_ref() else {
        clear_tiles(&mut commands, &mut state);
        return;
    };
    let Some(blades_per_m2) = blades_per_m2(density) else {
        clear_tiles(&mut commands, &mut state);
        state.bound_density = Some(density);
        return;
    };

    // (Re)bake the shared meshes when density changes or on first use — the only
    // place grass meshes are (re)allocated, never per-frame.
    let density_changed = state.bound_density != Some(density);
    if density_changed || state.variants.is_empty() {
        state.variants = (0..GRASS_LAYOUT_COUNT)
            .map(|layout| {
                meshes.add(mesh_from_blades(&generate_layout_blades(
                    layout,
                    blades_per_m2,
                )))
            })
            .collect();
    }
    if density_changed || state.bound_world_version != runtime.world_version {
        clear_tiles(&mut commands, &mut state);
    }
    state.bound_world_version = runtime.world_version;
    state.bound_density = Some(density);

    let Some(view) = runtime.local_view() else {
        return;
    };
    let (px, pz) = (view.position.x, view.position.z);

    // The material is binding-free and the fade is a shader constant, so it never
    // changes — create it once.
    let material = state
        .material
        .get_or_insert_with(|| materials.add(grass_material()))
        .clone();

    let GrassState {
        tiles, variants, ..
    } = &mut *state;

    let radius = GRASS_RADIUS_M;
    let radius_sq = radius * radius;
    // One tile of hysteresis so standing near a boundary doesn't thrash tiles.
    let keep_sq = (radius + GRASS_TILE_M) * (radius + GRASS_TILE_M);

    // 1. Despawn tiles that left the keep radius.
    tiles.retain(|&(tx, tz), slot| {
        let (cx, cz) = tile_center(tx, tz);
        let keep = (cx - px).powi(2) + (cz - pz).powi(2) <= keep_sq;
        if !keep && let Some(entity) = slot {
            commands.entity(*entity).despawn();
        }
        keep
    });

    // 2. Spawn newly in-range tiles (budgeted).
    let floor_half = (world.floor_size * 0.5 - GRASS_EDGE_MARGIN_M).max(0.0);
    let radius_tiles = (radius / GRASS_TILE_M).ceil() as i32 + 1;
    let cam_tx = (px / GRASS_TILE_M).floor() as i32;
    let cam_tz = (pz / GRASS_TILE_M).floor() as i32;

    let mut budget = MAX_GRASS_TILE_SPAWNS_PER_FRAME;
    'fill: for tx in (cam_tx - radius_tiles)..=(cam_tx + radius_tiles) {
        for tz in (cam_tz - radius_tiles)..=(cam_tz + radius_tiles) {
            if budget == 0 {
                break 'fill;
            }
            if tiles.contains_key(&(tx, tz)) {
                continue;
            }
            let (cx, cz) = tile_center(tx, tz);
            if (cx - px).powi(2) + (cz - pz).powi(2) > radius_sq {
                continue;
            }

            if !tile_is_plantable(tx, tz, floor_half, world) {
                tiles.insert((tx, tz), None);
                continue;
            }
            budget -= 1;

            let seed = tile_seed(tx, tz);
            let layout = (seed % GRASS_LAYOUT_COUNT as u64) as usize;
            let yaw = ((seed >> 8) % 4) as f32 * std::f32::consts::FRAC_PI_2;
            let entity = commands
                .spawn((
                    Name::new(format!("Grass Tile {tx},{tz}")),
                    GrassTile,
                    Mesh3d(variants[layout].clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::from_xyz(cx, 0.0, cz).with_rotation(Quat::from_rotation_y(yaw)),
                    Visibility::Visible,
                    // Grass never casts shadows (too many tiny casters), and has
                    // NO `VisibilityRange` — see the module docs.
                    NotShadowCaster,
                ))
                .id();
            tiles.insert((tx, tz), Some(entity));
        }
    }
}

/// Despawn every live tile and forget them. Keeps the cached material + variant
/// meshes + binding fields (the caller updates those).
fn clear_tiles(commands: &mut Commands, state: &mut GrassState) {
    for (_, slot) in state.tiles.drain() {
        if let Some(entity) = slot {
            commands.entity(entity).despawn();
        }
    }
}

fn grass_material() -> GrassMaterial {
    ExtendedMaterial {
        base: StandardMaterial {
            // Vertex colours carry the green gradient; the base colour passes
            // them through. Matte, near-zero reflectance — grass has no sheen.
            base_color: Color::WHITE,
            perceptual_roughness: 0.95,
            reflectance: 0.04,
            // Thin ribbons seen from any angle → render both faces. `double_sided`
            // stays false so both faces keep the baked upward normal (lit-from-
            // above, no orbit flicker).
            cull_mode: None,
            double_sided: false,
            ..default()
        },
        extension: GrassWindExtension {},
    }
}

/// Whether a tile should grow grass: fully inside the playable floor and not
/// overlapping any solid block (rare interior; mostly the perimeter walls).
fn tile_is_plantable(tx: i32, tz: i32, floor_half: f32, world: &WorldData) -> bool {
    let (cx, cz) = tile_center(tx, tz);
    let half = GRASS_TILE_M * 0.5;
    if cx.abs() + half > floor_half || cz.abs() + half > floor_half {
        return false;
    }
    let (min_x, max_x) = (cx - half, cx + half);
    let (min_z, max_z) = (cz - half, cz + half);
    !world
        .blocks
        .iter()
        .any(|b| block_overlaps(b, min_x, max_x, min_z, max_z))
}

fn block_overlaps(block: &WorldBlock, min_x: f32, max_x: f32, min_z: f32, max_z: f32) -> bool {
    let bx = block.center.x;
    let bz = block.center.z;
    let hx = block.half_extents.x;
    let hz = block.half_extents.z;
    bx - hx <= max_x && bx + hx >= min_x && bz - hz <= max_z && bz + hz >= min_z
}

fn tile_center(tx: i32, tz: i32) -> (f32, f32) {
    (
        tx as f32 * GRASS_TILE_M + GRASS_TILE_M * 0.5,
        tz as f32 * GRASS_TILE_M + GRASS_TILE_M * 0.5,
    )
}

/// Stable per-tile hash. Folds the two signed tile coords into one `u64` then
/// runs it through `splitmix64`. No world seed — placement is seed-free.
fn tile_seed(tx: i32, tz: i32) -> u64 {
    let folded = ((tx as u32 as u64) << 32) | (tz as u32 as u64);
    splitmix64(folded ^ 0x9E37_79B9_7F4A_7C15)
}

/// Pull the next pseudo-random `f32` in `[0, 1)` from a splitmix64 stream.
fn next_unit(state: &mut u64) -> f32 {
    *state = splitmix64(*state);
    ((*state >> 40) as f32) / ((1u64 << 24) as f32)
}

/// Base (dark) and tip (light) green for a blade, before the per-blade shade /
/// warmth tweaks. Tuned to sit at/just below the ground tone (`WORLD_COLOR`).
const BLADE_BASE: [f32; 3] = [0.08, 0.18, 0.07];
const BLADE_TIP: [f32; 3] = [0.19, 0.34, 0.13];

/// One generated blade.
struct Blade {
    base: Vec2,
    yaw: f32,
    height: f32,
    half_width: f32,
    bend: Vec2,
    base_color: [f32; 4],
    tip_color: [f32; 4],
    /// Per-blade random in `[0, 1)`, stored in every vertex's `uv.x` as the
    /// shader's distance-dither key (whole-blade keep/drop).
    dither: f32,
}

/// Deterministically scatter one layout's blades, centred on the origin spanning
/// `[-GRASS_TILE_M/2, GRASS_TILE_M/2]` so a cardinal rotation about the tile
/// centre maps the square onto itself (no seams).
fn generate_layout_blades(layout: usize, blades_per_m2: f32) -> Vec<Blade> {
    let half = GRASS_TILE_M * 0.5;
    let candidates = (blades_per_m2 * GRASS_TILE_M * GRASS_TILE_M).round() as u32;
    let mut rng = splitmix64(GRASS_LAYOUT_SEED ^ (layout as u64).wrapping_mul(0x100_0001));

    let mut blades = Vec::new();
    for _ in 0..candidates {
        let lx = next_unit(&mut rng) * GRASS_TILE_M - half;
        let lz = next_unit(&mut rng) * GRASS_TILE_M - half;

        // Clumping: thin blades where the noise is low so grass grows in patches.
        let clump = fbm(
            GRASS_CLUMP_SEED ^ layout as u64,
            (lx + half) * 0.18,
            (lz + half) * 0.18,
            1.0,
            3,
        );
        let keep_chance = 0.25 + 0.75 * clump;
        if next_unit(&mut rng) > keep_chance {
            continue;
        }

        let yaw = next_unit(&mut rng) * std::f32::consts::TAU;
        let height = 0.16 + next_unit(&mut rng) * 0.20;
        let half_width = 0.022 + next_unit(&mut rng) * 0.016;
        let lean = (clump - 0.5) * 0.12;
        let lean_dir = next_unit(&mut rng) * std::f32::consts::TAU;
        let bend = Vec2::new(lean_dir.cos() * lean, lean_dir.sin() * lean);
        // Darken-only shade + small warm/cool hue jitter so the field isn't flat.
        let shade = 0.7 + next_unit(&mut rng) * 0.3;
        let warm = next_unit(&mut rng) * 2.0 - 1.0;
        let (base_color, tip_color) = blade_colors(shade, warm);
        let dither = next_unit(&mut rng);

        blades.push(Blade {
            base: Vec2::new(lx, lz),
            yaw,
            height,
            half_width,
            bend,
            base_color,
            tip_color,
            dither,
        });
    }

    if blades.is_empty() {
        let (base_color, tip_color) = blade_colors(0.9, 0.0);
        blades.push(Blade {
            base: Vec2::ZERO,
            yaw: 0.0,
            height: 0.2,
            half_width: 0.03,
            bend: Vec2::ZERO,
            base_color,
            tip_color,
            dither: 0.0,
        });
    }
    blades
}

fn mesh_from_blades(blades: &[Blade]) -> Mesh {
    let mut builder = BladeMeshBuilder::default();
    for blade in blades {
        builder.push_blade(blade);
    }
    builder.build()
}

/// Base/tip blade colours for a `shade` (darken multiplier, ≤ 1.0) and `warm`
/// hue jitter in `[-1, 1]` (positive = warmer/yellower). Alpha carries the sway
/// weight (base 0, tip 1) for the wind shader.
fn blade_colors(shade: f32, warm: f32) -> ([f32; 4], [f32; 4]) {
    let tint = |rgb: [f32; 3], sway: f32| {
        [
            (rgb[0] * shade + warm * 0.05).clamp(0.0, 1.0),
            (rgb[1] * shade + warm * 0.01).clamp(0.0, 1.0),
            (rgb[2] * shade - warm * 0.03).clamp(0.0, 1.0),
            sway,
        ]
    };
    (tint(BLADE_BASE, 0.0), tint(BLADE_TIP, 1.0))
}

#[derive(Default)]
struct BladeMeshBuilder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl BladeMeshBuilder {
    /// Append one tapered blade quad (base → near-point tip).
    fn push_blade(&mut self, blade: &Blade) {
        let (s, c) = blade.yaw.sin_cos();
        let ax = Vec2::new(c, s);
        let top_width = blade.half_width * 0.18;
        let base = blade.base;

        let bl = [
            base.x - ax.x * blade.half_width,
            0.0,
            base.y - ax.y * blade.half_width,
        ];
        let br = [
            base.x + ax.x * blade.half_width,
            0.0,
            base.y + ax.y * blade.half_width,
        ];
        let tcx = base.x + blade.bend.x;
        let tcz = base.y + blade.bend.y;
        let tl = [tcx - ax.x * top_width, blade.height, tcz - ax.y * top_width];
        let tr = [tcx + ax.x * top_width, blade.height, tcz + ax.y * top_width];

        let base_index = self.positions.len() as u32;
        self.positions.extend_from_slice(&[bl, br, tr, tl]);
        // Upward normals: grass reads as lit-from-above, blending with the ground.
        self.normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 4]);
        self.colors.extend_from_slice(&[
            blade.base_color,
            blade.base_color,
            blade.tip_color,
            blade.tip_color,
        ]);
        // Same per-blade dither key on every vertex → whole-blade keep/drop in
        // the shader.
        self.uvs.extend_from_slice(&[[blade.dither, 0.0]; 4]);
        self.indices.extend_from_slice(&[
            base_index,
            base_index + 1,
            base_index + 2,
            base_index,
            base_index + 2,
            base_index + 3,
        ]);
    }

    fn build(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Vec3Net;
    use crate::world::BlockKind;
    use bevy::mesh::VertexAttributeValues;

    fn vertex_count(mesh: &Mesh) -> usize {
        match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(VertexAttributeValues::Float32x3(p)) => p.len(),
            _ => 0,
        }
    }

    fn world_with(floor_size: f32, blocks: Vec<WorldBlock>) -> WorldData {
        WorldData {
            floor_size,
            blocks,
            resource_nodes: Vec::new(),
        }
    }

    #[test]
    fn off_density_has_no_blades() {
        assert!(blades_per_m2(GrassDensity::Off).is_none());
        assert!(blades_per_m2(GrassDensity::Low).is_some());
    }

    #[test]
    fn next_unit_stays_in_unit_range() {
        let mut state = 12345;
        for _ in 0..10_000 {
            let v = next_unit(&mut state);
            assert!((0.0..1.0).contains(&v), "rng escaped [0,1): {v}");
        }
    }

    #[test]
    fn tile_seed_is_stable_and_distinct() {
        assert_eq!(tile_seed(3, -7), tile_seed(3, -7));
        assert_ne!(tile_seed(3, -7), tile_seed(-7, 3));
        assert_ne!(tile_seed(0, 0), tile_seed(0, 1));
    }

    #[test]
    fn layout_mesh_is_deterministic_and_nonempty() {
        let a = mesh_from_blades(&generate_layout_blades(3, 11.0));
        let b = mesh_from_blades(&generate_layout_blades(3, 11.0));
        assert!(vertex_count(&a) > 0);
        assert_eq!(vertex_count(&a), vertex_count(&b));
        assert_eq!(vertex_count(&a) % 4, 0, "four verts per blade quad");
    }

    #[test]
    fn higher_density_grows_more_grass() {
        let low = generate_layout_blades(0, 4.0).len();
        let high = generate_layout_blades(0, 17.0).len();
        assert!(high > low, "higher density places more blades");
    }

    #[test]
    fn tile_inside_floor_is_plantable() {
        let world = world_with(1000.0, vec![]);
        assert!(tile_is_plantable(
            0,
            0,
            1000.0 * 0.5 - GRASS_EDGE_MARGIN_M,
            &world
        ));
    }

    #[test]
    fn tile_past_floor_edge_is_bare() {
        let world = world_with(40.0, vec![]);
        let floor_half = 40.0 * 0.5 - GRASS_EDGE_MARGIN_M;
        assert!(!tile_is_plantable(50, 50, floor_half, &world));
    }

    #[test]
    fn tile_over_a_block_is_bare() {
        let (cx, cz) = tile_center(0, 0);
        let block = WorldBlock {
            center: Vec3Net::new(cx, 0.0, cz),
            half_extents: Vec3Net::new(1.0, 2.0, 1.0),
            kind: BlockKind::Stone,
        };
        let world = world_with(1000.0, vec![block]);
        let floor_half = 1000.0 * 0.5 - GRASS_EDGE_MARGIN_M;
        assert!(!tile_is_plantable(0, 0, floor_half, &world));
        assert!(tile_is_plantable(5, 5, floor_half, &world));
    }

    #[test]
    fn blade_bakes_sway_and_dither() {
        let (base_color, tip_color) = blade_colors(0.9, 0.0);
        let mut b = BladeMeshBuilder::default();
        b.push_blade(&Blade {
            base: Vec2::new(1.0, 1.0),
            yaw: 0.3,
            height: 0.3,
            half_width: 0.03,
            bend: Vec2::ZERO,
            base_color,
            tip_color,
            dither: 0.42,
        });
        // Sway weight in colour alpha: base 0, tip 1.
        assert_eq!(b.colors[0][3], 0.0);
        assert_eq!(b.colors[2][3], 1.0);
        // Dither key in uv.x, identical on all four verts (whole-blade decision).
        assert!(b.uvs.iter().all(|uv| uv[0] == 0.42));
        assert!(b.positions[2][1] > b.positions[0][1], "tip above base");
    }
}
