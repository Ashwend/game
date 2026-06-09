//! Procedural detail grass, a client-only cosmetic ground layer streamed in
//! tiles around the camera and drawn with a custom GPU-instancing pipeline.
//!
//! Placement is **seed-free**: which layout + rotation each tile gets is a pure
//! hash of its world-XZ tile coords, deterministic without any server data and
//! identical in singleplayer and multiplayer. Nothing here touches gameplay,
//! collision, the protocol, or the dedicated server.
//!
//! ## Rendering: GPU instancing
//!
//! One shared cubic-Bézier blade mesh ([`build_instanced_blade_mesh`]) is drawn
//! thousands of times from a per-blade instance buffer, so density is cheap (each
//! straw is ~no extra geometry). The pipeline + shader live in [`instancing`];
//! the shader hand-builds a `PbrInput` and calls `apply_pbr_lighting`, so grass
//! is lit by the same sun + atmosphere IBL as the rest of the scene. It adds:
//! - **Vertex wind sway**, tips ride a world-space wave weighted by the per-vertex
//!   sway weight (vertex-colour alpha, 0 base → 1 tip), so blades bend.
//! - **Fragment radial dither**, whole blades drop out with distance (key = a
//!   stable per-instance random), thinning the field into smooth rings (no hard
//!   despawn edge, no square tile bands).
//!
//! All visible blades are kept in **one** entity's combined instance buffer; see
//! [`GrassState`] and [`instancing`] for why one-entity-one-buffer (Bevy's
//! auto-instancing clumps many entities that share a mesh).
//!
//! The harvestable **hay-grass** node still uses the older [`GrassMaterial`]
//! (`grass.wgsl`) baked-clump path, it's one located, pickable plant per node,
//! not a density problem.
//!
//! ## Two render traps this design avoids
//!
//! 1. **No per-tile mesh assets.** Creating/freeing a mesh per tile as it streams
//!    makes Bevy's `MeshAllocator` repack shared "general slabs", corrupting other
//!    small meshes in them. So we build the blade mesh **once** (rebuilt only when
//!    density changes) and stream only instance data.
//! 2. **No `VisibilityRange` on grass.** Bevy rebuilds one global visibility-range
//!    table whenever any `VisibilityRange` entity is added/removed; the trees use
//!    it for LOD, so a grass tile streaming every step would clobber that table
//!    and flicker whole regions of trees. The shader's distance fade replaces it.

use std::collections::HashMap;

use bevy::{
    camera::visibility::NoFrustumCulling,
    pbr::{ExtendedMaterial, MaterialExtension},
    prelude::*,
    render::{render_resource::AsBindGroup, sync_world::SyncToRenderWorld},
    shader::ShaderRef,
};

mod instancing;

pub(crate) use instancing::GrassInstancingPlugin;
use instancing::{InstanceData, InstanceMaterialData};

use super::components::WorldGeometry;
use super::mesh::builder::{GrassBlade, GrassBladeMesh, grass_blade_colors};
use crate::{
    app::state::{ClientRuntime, ClientSettings, GrassDensity},
    world::{WorldBlock, WorldData, fbm, splitmix64},
};

/// Height (m) the shared instanced blade mesh is baked at. Per-blade
/// `height_scale` in the instance buffer scales relative to this.
const BLADE_REF_HEIGHT: f32 = 0.4;

/// Embedded path of the grass shader (see [`crate::app::embedded_asset_path`],
/// the same `embedded://` scheme, but a `&'static str` because [`ShaderRef`]
/// needs one).
const GRASS_SHADER_PATH: &str = "embedded://shaders/grass.wgsl";

/// Side length of a grass streaming tile / variant mesh, in metres.
const GRASS_TILE_M: f32 = 8.0;

/// Keep grass this far inside the floor edge so tiles never clip the perimeter
/// walls or hang past the ground plane.
const GRASS_EDGE_MARGIN_M: f32 = 3.0;

/// Tiles whose instances are generated per frame. Capped so the first fill on
/// entering a world drains over a few frames instead of one spike (each tile is
/// hundreds-to-thousands of instance records to build + concatenate).
const MAX_GRASS_TILE_SPAWNS_PER_FRAME: usize = 12;

/// Distinct blade layouts to bake. Each tile picks one by hash and rotates it to
/// one of four cardinal angles, giving `4 × COUNT` permutations, plenty to hide
/// tiling across an open field.
const GRASS_LAYOUT_COUNT: usize = 16;

/// Fixed seeds, decoupled from any world seed (placement is seed-free).
const GRASS_CLUMP_SEED: u64 = 0x6A09_E667_F3BC_C909;
const GRASS_LAYOUT_SEED: u64 = 0xBB67_AE85_84CA_A73B;

/// The grass material: StandardMaterial PBR + the wind/dither shader extension.
pub(crate) type GrassMaterial = ExtendedMaterial<StandardMaterial, GrassWindExtension>;

/// Shared handle to the single [`GrassMaterial`] instance. Created eagerly at
/// scene setup and used by the harvestable hay-grass node (the cosmetic detail
/// grass moved to the GPU-instanced [`instancing`] pipeline). The material is
/// binding-free, so one instance suffices.
#[derive(Resource, Clone)]
pub(crate) struct GrassMaterialHandle(pub(crate) Handle<GrassMaterial>);

/// Shader extension that adds the wind + distance-fade behaviour. **Deliberately
/// binding-free**: it carries no uniform/texture, only the shader override.
/// `ExtendedMaterial`'s bind-group merge with the bindless `StandardMaterial`
/// drops a `@binding(100)` extension uniform from the pipeline layout on Metal
/// (crash: "binding 100 missing from pipeline layout"), so all shader tuning is
/// compile-time constants in `grass.wgsl` instead, the trade-off being a fixed
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

/// Camera-relative radius (m) within which grass tiles are kept loaded. A hair
/// above the instanced shader's `FADE_END` (`grass_instanced.wgsl`, 46 m) so
/// grass is fully faded before a tile drops out (no pop).
const GRASS_RADIUS_M: f32 = 47.0;

/// Per-tier patch density (grass *tufts* per square metre, before clumping). The
/// cosmetic grass is scattered tufts, not a continuous carpet, so this counts
/// patches; each patch fans [`PATCH_BLADES_MIN`]..+[`PATCH_BLADES_SPAN`] straws.
fn patch_density_per_m2(density: GrassDensity) -> Option<f32> {
    match density {
        GrassDensity::Off => None,
        GrassDensity::Low => Some(0.22),
        GrassDensity::Medium => Some(0.45),
        GrassDensity::High => Some(0.8),
    }
}

/// Marker for the grass field entity.
#[derive(Component)]
pub(crate) struct GrassTile;

/// Streaming bookkeeping for the detail grass.
///
/// All visible blades live in **one** entity's instance buffer (the custom
/// instancing pipeline is built for one mesh + one instance buffer per draw;
/// many entities sharing a mesh collide with Bevy's auto-instancing). The streamer
/// maintains a per-tile map of world-space instances and rebuilds the single
/// combined buffer whenever the loaded set changes.
#[derive(Resource, Default)]
pub(crate) struct GrassState {
    /// Loaded tiles by `(tile_x, tile_z)`: `Some(world instances)` if planted,
    /// `None` if permanently bare (off the floor or covering a block).
    tiles: HashMap<(i32, i32), Option<Vec<InstanceData>>>,
    /// The single shared blade mesh. Built once per density, never freed.
    blade_mesh: Option<Handle<Mesh>>,
    /// Pre-computed per-layout **tile-local** instance lists (one per layout).
    /// Each tile clones the matching layout, transformed to world space (see
    /// [`tile_world_instances`]). Rebuilt only when density changes.
    layouts: Vec<Vec<InstanceData>>,
    /// The one entity holding every visible blade as a combined instance buffer.
    field_entity: Option<Entity>,
    /// Set when `tiles` changed so the combined buffer is rebuilt this frame.
    dirty: bool,
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
    mut state: ResMut<GrassState>,
) {
    let density = settings.graphics.grass_density;

    // Only stream grass inside a live world; clear the field otherwise.
    let Some(world) = runtime.world.as_ref() else {
        clear_field(&mut commands, &mut state);
        return;
    };
    let Some(patches_per_m2) = patch_density_per_m2(density) else {
        clear_field(&mut commands, &mut state);
        state.bound_density = Some(density);
        return;
    };

    // (Re)build the shared blade mesh + per-layout instance lists when density
    // changes or on first use, the only place these are (re)allocated.
    let density_changed = state.bound_density != Some(density);
    if density_changed || state.blade_mesh.is_none() {
        state.blade_mesh = Some(meshes.add(build_instanced_blade_mesh()));
        state.layouts = (0..GRASS_LAYOUT_COUNT)
            .map(|layout| generate_layout_instances(layout, patches_per_m2))
            .collect();
    }
    if density_changed || state.bound_world_version != runtime.world_version {
        clear_field(&mut commands, &mut state);
    }
    state.bound_world_version = runtime.world_version;
    state.bound_density = Some(density);

    let Some(view) = runtime.local_view() else {
        return;
    };
    let (px, pz) = (view.position.x, view.position.z);

    let blade_mesh = state
        .blade_mesh
        .clone()
        .expect("blade mesh built above when density is on");

    let GrassState {
        tiles,
        layouts,
        field_entity,
        dirty,
        ..
    } = &mut *state;

    let radius = GRASS_RADIUS_M;
    let radius_sq = radius * radius;
    // One tile of hysteresis so standing near a boundary doesn't thrash tiles.
    let keep_sq = (radius + GRASS_TILE_M) * (radius + GRASS_TILE_M);

    // 1. Drop tiles that left the keep radius.
    let before = tiles.len();
    tiles.retain(|&(tx, tz), _| {
        let (cx, cz) = tile_center(tx, tz);
        (cx - px).powi(2) + (cz - pz).powi(2) <= keep_sq
    });
    if tiles.len() != before {
        *dirty = true;
    }

    // 2. Load newly in-range tiles (budgeted) into the per-tile map.
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
            // World-space instances (tile centre + cardinal rotation baked in),
            // so the shader needs no per-entity model matrix.
            tiles.insert(
                (tx, tz),
                Some(tile_world_instances(&layouts[layout], cx, cz, yaw)),
            );
            *dirty = true;
        }
    }

    // 3. Rebuild the single combined instance buffer when the loaded set changed.
    if *dirty {
        *dirty = false;
        let combined: Vec<InstanceData> = tiles
            .values()
            .filter_map(|slot| slot.as_ref())
            .flat_map(|blades| blades.iter().copied())
            .collect();
        update_grass_field(&mut commands, field_entity, &blade_mesh, combined, px, pz);
    }
}

/// Spawn or refresh the single grass-field entity with the combined instance
/// buffer. Despawns it when the field is empty (a 0-length GPU buffer is invalid).
fn update_grass_field(
    commands: &mut Commands,
    field_entity: &mut Option<Entity>,
    blade_mesh: &Handle<Mesh>,
    combined: Vec<InstanceData>,
    px: f32,
    pz: f32,
) {
    if combined.is_empty() {
        if let Some(entity) = field_entity.take() {
            commands.entity(entity).despawn();
        }
        return;
    }
    match field_entity {
        Some(entity) => {
            commands.entity(*entity).insert((
                InstanceMaterialData(combined),
                Transform::from_xyz(px, 0.0, pz),
            ));
        }
        None => {
            *field_entity = Some(
                commands
                    .spawn((
                        Name::new("Grass Field"),
                        GrassTile,
                        Mesh3d(blade_mesh.clone()),
                        InstanceMaterialData(combined),
                        // Transform (≈ camera) only feeds transparent-sort
                        // distance; blade positions are already world-space.
                        Transform::from_xyz(px, 0.0, pz),
                        Visibility::Visible,
                        // Blades span the whole field but the mesh Aabb is one
                        // blade at the origin, so skip built-in frustum culling.
                        NoFrustumCulling,
                        // No `Material`, so opt into render-world sync explicitly
                        // (the instancing extract needs a `RenderEntity`).
                        SyncToRenderWorld,
                    ))
                    .id(),
            );
        }
    }
}

/// Blade density for the static menu-backdrop grass carpet (Medium-ish).
const MENU_GRASS_BLADES_PER_M2: f32 = 11.0;

/// Spawn a fixed patch of detail grass for the main-menu backdrop, tagged
/// [`WorldGeometry`] so it's torn down with the rest of the backdrop on scene
/// change. Uses the shared wind [`GrassMaterial`] + the same blade meshes as the
/// in-game grass, but as a static patch, the menu camera barely drifts, so
/// streaming isn't needed and the shader's radial fade thins the far edge.
///
/// The tile range covers the camera's visible foreground/midground (camera near
/// `(-5.8, 7.2)` looking toward `(0.4, -3.6)`); tiles past the fade radius are
/// still spawned but dithered away by the shader.
pub(crate) fn spawn_menu_grass(commands: &mut Commands, meshes: &mut Assets<Mesh>) {
    let blade_mesh = meshes.add(build_instanced_blade_mesh());
    let layouts: Vec<Vec<InstanceData>> = (0..GRASS_LAYOUT_COUNT)
        .map(|layout| generate_layout_instances(layout, MENU_GRASS_BLADES_PER_M2))
        .collect();

    // One combined instance buffer over the visible ground band (8 m tiles).
    let mut combined = Vec::new();
    for tx in -2..=4 {
        for tz in -3..=0 {
            let (cx, cz) = tile_center(tx, tz);
            let seed = tile_seed(tx, tz);
            let layout = (seed % GRASS_LAYOUT_COUNT as u64) as usize;
            let yaw = ((seed >> 8) % 4) as f32 * std::f32::consts::FRAC_PI_2;
            combined.extend(tile_world_instances(&layouts[layout], cx, cz, yaw));
        }
    }
    if combined.is_empty() {
        return;
    }
    commands.spawn((
        Name::new("Menu Grass"),
        WorldGeometry,
        Mesh3d(blade_mesh),
        InstanceMaterialData(combined),
        Transform::default(),
        Visibility::Visible,
        NoFrustumCulling,
        SyncToRenderWorld,
    ));
}

/// Despawn the grass field entity and forget all loaded tiles. Keeps the cached
/// blade mesh + layouts (the caller updates those).
fn clear_field(commands: &mut Commands, state: &mut GrassState) {
    if let Some(entity) = state.field_entity.take() {
        commands.entity(entity).despawn();
    }
    state.tiles.clear();
    state.dirty = false;
}

pub(crate) fn grass_material() -> GrassMaterial {
    ExtendedMaterial {
        base: StandardMaterial {
            // Vertex colours carry the green gradient; the base colour passes
            // them through. Matte, near-zero reflectance, grass has no sheen.
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
/// runs it through `splitmix64`. No world seed, placement is seed-free.
fn tile_seed(tx: i32, tz: i32) -> u64 {
    let folded = ((tx as u32 as u64) << 32) | (tz as u32 as u64);
    splitmix64(folded ^ 0x9E37_79B9_7F4A_7C15)
}

/// Pull the next pseudo-random `f32` in `[0, 1)` from a splitmix64 stream.
fn next_unit(state: &mut u64) -> f32 {
    *state = splitmix64(*state);
    ((*state >> 40) as f32) / ((1u64 << 24) as f32)
}

/// The single blade mesh every tile instances: one cubic-Bézier arch at the
/// origin, baked at [`BLADE_REF_HEIGHT`] with **neutral** colour (shade 1, warm
/// 0). Per-instance `height_scale`, `yaw`, and `shade`/`warm` are applied in the
/// shader, so all the per-blade variety lives in the instance buffer, not the
/// geometry. The blade leans along +Z (perpendicular to its width axis) so the
/// arch bows over its face; the instance yaw then spins the whole thing.
fn build_instanced_blade_mesh() -> Mesh {
    let (base_color, tip_color) = grass_blade_colors(1.0, 0.0);
    let mut builder = GrassBladeMesh::default();
    builder.push_blade(&GrassBlade {
        base: Vec2::ZERO,
        yaw: 0.0,
        height: BLADE_REF_HEIGHT,
        half_width: 0.016,
        // Lean + a gentle arch, both aimed away from the tuft centre by the
        // instance `yaw`, so each straw curves outward into the rosette. `flex` is
        // a moderate bow (well below the old 0.22 that read as "massively curvy").
        lean: Vec2::new(0.0, BLADE_REF_HEIGHT * 0.5),
        flex: 0.14,
        base_color,
        tip_color,
        // Unused for instanced grass (the dither key is per-instance).
        dither: 0.0,
    });
    builder.build()
}

/// Straws per tuft (minimum + random span on top): a fuller 7-10 straw rosette.
const PATCH_BLADES_MIN: u32 = 7;
const PATCH_BLADES_SPAN: u32 = 4;
/// Base footprint radius of a tuft (m): kept tight so straws share a centre and
/// the outward lean fans the *tips* into a clear rosette.
const PATCH_RADIUS_M: f32 = 0.08;

/// Deterministically scatter one layout's **tufts** into tile-local instance data,
/// centred on the origin spanning `[-GRASS_TILE_M/2, GRASS_TILE_M/2]` so a cardinal
/// rotation about the tile centre maps the square onto itself (no seams). Each tuft
/// is a few straws fanning outward from its centre (instance `yaw` = the radial
/// direction, so the baked lean points away from centre). fBm clumping gathers
/// tufts into loose meadow clusters with bare ground between.
fn generate_layout_instances(layout: usize, patches_per_m2: f32) -> Vec<InstanceData> {
    let half = GRASS_TILE_M * 0.5;
    let patch_candidates = (patches_per_m2 * GRASS_TILE_M * GRASS_TILE_M).round() as u32;
    let mut rng = splitmix64(GRASS_LAYOUT_SEED ^ (layout as u64).wrapping_mul(0x100_0001));

    let mut out = Vec::new();
    for _ in 0..patch_candidates {
        let cx = next_unit(&mut rng) * GRASS_TILE_M - half;
        let cz = next_unit(&mut rng) * GRASS_TILE_M - half;

        // Clumping: keep tufts where the noise is high → loose meadow clusters.
        let clump = fbm(
            GRASS_CLUMP_SEED ^ layout as u64,
            (cx + half) * 0.18,
            (cz + half) * 0.18,
            1.0,
            3,
        );
        if next_unit(&mut rng) > 0.35 + 0.65 * clump {
            continue;
        }

        // One tuft. Whole-patch shade/warm so the straws read as one plant.
        let blades = PATCH_BLADES_MIN + (next_unit(&mut rng) * PATCH_BLADES_SPAN as f32) as u32;
        let patch_shade = 0.9 + next_unit(&mut rng) * 0.1;
        // Mild hue jitter, kept small so the now-dim (ground-matched) colours
        // don't tip warm/yellow and stand out.
        let patch_warm = next_unit(&mut rng) * 0.3 - 0.16;
        let start = next_unit(&mut rng) * std::f32::consts::TAU;
        for i in 0..blades {
            // Even fan around the centre with a little jitter.
            let az = start
                + (i as f32 / blades as f32) * std::f32::consts::TAU
                + (next_unit(&mut rng) - 0.5) * 0.5;
            let r = PATCH_RADIUS_M * next_unit(&mut rng).sqrt();
            let bx = cx + az.cos() * r;
            let bz = cz + az.sin() * r;
            // Aim the blade's baked +Z lean/arch radially *outward* from the tuft
            // centre. The shader maps local +Z to world `(sin yaw, cos yaw)` and
            // the radial direction is `(cos az, sin az)`, so `yaw = π/2 - az`.
            // (Using `az` directly leans tangentially, a pinwheel, which read as
            // straws curving inward/sideways.)
            let yaw = std::f32::consts::FRAC_PI_2 - az + (next_unit(&mut rng) - 0.5) * 0.35;
            let height = 0.26 + next_unit(&mut rng) * 0.22;
            let height_scale = height / BLADE_REF_HEIGHT;
            let shade = patch_shade * (0.92 + next_unit(&mut rng) * 0.08);
            let dither = next_unit(&mut rng);
            out.push(InstanceData {
                a: [bx, bz, 0.0, height_scale],
                b: [yaw, shade, patch_warm, dither],
            });
        }
    }
    out
}

/// Transform a layout's tile-local instances into world space for a tile at
/// `(cx, cz)` rotated by `tile_yaw` (one of the four cardinal angles). Rotating
/// the local XZ + folding `tile_yaw` into each blade's spin reproduces the old
/// per-tile `Transform` rotation, but baked into the instance buffer so the
/// shader needs no model matrix.
fn tile_world_instances(
    local: &[InstanceData],
    cx: f32,
    cz: f32,
    tile_yaw: f32,
) -> Vec<InstanceData> {
    let (ts, tc) = tile_yaw.sin_cos();
    local
        .iter()
        .map(|inst| {
            let lx = inst.a[0];
            let lz = inst.a[1];
            InstanceData {
                a: [
                    cx + tc * lx + ts * lz,
                    cz - ts * lx + tc * lz,
                    inst.a[2],
                    inst.a[3],
                ],
                b: [inst.b[0] + tile_yaw, inst.b[1], inst.b[2], inst.b[3]],
            }
        })
        .collect()
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
    fn off_density_has_no_grass() {
        assert!(patch_density_per_m2(GrassDensity::Off).is_none());
        assert!(patch_density_per_m2(GrassDensity::Low).is_some());
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
    fn layout_instances_are_deterministic_and_nonempty() {
        let a = generate_layout_instances(3, 11.0);
        let b = generate_layout_instances(3, 11.0);
        assert!(!a.is_empty());
        assert_eq!(a.len(), b.len());
        assert_eq!(a[0].a, b[0].a, "placement is seed-free deterministic");
        assert_eq!(a[0].b, b[0].b);
        // The shared instanced blade mesh has real geometry.
        assert!(vertex_count(&build_instanced_blade_mesh()) > 0);
    }

    #[test]
    fn higher_density_grows_more_grass() {
        let low = generate_layout_instances(0, 4.0).len();
        let high = generate_layout_instances(0, 17.0).len();
        assert!(high > low, "higher density places more blades");
    }

    #[test]
    fn tile_world_instances_translate_and_spin() {
        let local = generate_layout_instances(1, 11.0);
        let world = tile_world_instances(&local, 100.0, -50.0, std::f32::consts::FRAC_PI_2);
        assert_eq!(local.len(), world.len());
        // Local origin-ish blade maps near the tile centre, and the tile yaw is
        // folded into each blade's spin.
        assert!((world[0].b[0] - (local[0].b[0] + std::f32::consts::FRAC_PI_2)).abs() < 1e-4);
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
}
