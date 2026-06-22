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
//! All visible blades are kept in **one** entity's combined instance buffer, drawn
//! whole with `NoFrustumCulling`; see [`GrassState`] and [`instancing`] for why
//! one-entity-one-buffer (Bevy's auto-instancing clumps many entities that share a
//! mesh, and per-region frustum culling made the field flicker chunk-by-chunk as the
//! camera moved, so we draw the whole loaded field as a single buffer).
//!
//! The harvestable **hay-grass** node is a normal `StandardMaterial` mesh (the
//! taller, straw-tinted [`super::mesh::builder`] tuft), spawned by the resource-node
//! system like any other crude clutter, not part of this cosmetic field.
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
    camera::visibility::NoFrustumCulling, prelude::*, render::sync_world::SyncToRenderWorld,
};

mod instancing;

pub(crate) use instancing::GrassInstancingPlugin;
use instancing::{InstanceData, InstanceMaterialData};

use super::components::WorldGeometry;
use super::mesh::builder::build_grass_card_mesh;
use crate::{
    app::state::{ClientRuntime, ClientSettings, GrassDensity},
    world::{ClassificationChannels, WorldBlock, WorldData, biome_blend_weights, fbm, splitmix64},
};

/// Height (m) the shared instanced blade mesh is baked at. Per-blade
/// `height_scale` in the instance buffer scales relative to this.
pub(super) const BLADE_REF_HEIGHT: f32 = 0.6;

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
const GRASS_HEIGHT_SEED: u64 = 0x3C6E_F372_FE94_F82B;
const GRASS_COLOR_SEED: u64 = 0xA54F_F53A_5F1D_36F1;
const GRASS_LEAN_SEED: u64 = 0x510E_527F_ADE6_82D1;

/// The shared grass-card mesh ([`build_instanced_blade_mesh`]) for the cosmetic
/// detail-grass field, built once at startup. Density-independent (density only
/// scales the instance *count*), so one mesh serves every tier; the streamer clones
/// this handle instead of rebuilding the mesh on density changes.
#[derive(Resource, Clone)]
pub(crate) struct GrassCardMesh(pub(crate) Handle<Mesh>);

/// Build the shared [`GrassCardMesh`] at startup. Run from [`GrassInstancingPlugin`].
pub(super) fn init_grass_card_mesh(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(GrassCardMesh(meshes.add(build_instanced_blade_mesh())));
}

/// Number of tuft variants packed into the grass card atlas
/// (`assets/textures/grass_atlas.png`, a 3x2 grid). Each blade picks one cell at
/// random; the shader (`grass_instanced.wgsl`) remaps the card UV into it.
const GRASS_ATLAS_CELLS: u32 = 6;

/// Camera-relative radius (m) within which grass tiles are kept loaded. Comfortably
/// above the instanced shader's `FADE_END` (`grass_instanced.wgsl`, 50 m) so a tile
/// is fully dithered out before it loads/drops, the cards just dissolve in/out with
/// distance rather than popping (no visible "spawning" as you walk).
const GRASS_RADIUS_M: f32 = 54.0;

/// Per-tier grass-CARD density (textured tuft cards per square metre, before the
/// fBm biome thinning). Each card is a whole textured tuft of ~10 visual blades.
/// Tuned sparse for the stylised anime art direction: a dense carpet fights the
/// clean toon ground + cel props, so the whole scale was pulled down (the old
/// "Low" of 1.5 is now the ceiling/`High`). The shader's distance dither does the
/// far thin-out, so density is uniform here (no CPU distance falloff).
fn blade_density_per_m2(density: GrassDensity) -> Option<f32> {
    match density {
        GrassDensity::Off => None,
        GrassDensity::Low => Some(0.6),
        GrassDensity::Medium => Some(1.0),
        GrassDensity::High => Some(1.5),
    }
}

/// Marker for the grass field entity.
#[derive(Component)]
pub(crate) struct GrassTile;

/// Streaming bookkeeping for the detail grass.
///
/// All visible blades live in **one** entity's instance buffer (the custom
/// instancing pipeline is built for one mesh + one instance buffer per draw; many
/// entities sharing a mesh collide with Bevy's auto-instancing). The streamer
/// maintains a per-tile map of world-space instances and rebuilds the single
/// combined buffer whenever the loaded set changes.
#[derive(Resource, Default)]
pub(crate) struct GrassState {
    /// Loaded tiles by `(tile_x, tile_z)`: `Some(world instances)` if planted,
    /// `None` if permanently bare (off the floor or covering a block).
    tiles: HashMap<(i32, i32), Option<Vec<InstanceData>>>,
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
    /// Camera tile the last full streaming scan ran for. While the camera stays
    /// in this tile (and no budgeted fill is still draining) the loaded set can't
    /// change, so the per-frame retain + radius rescan are skipped. Reset to
    /// `None` by [`clear_field`] (world / density change) to force a rescan.
    last_cam_tile: Option<(i32, i32)>,
    /// True when the last scan hit its per-frame spawn budget and tiles still
    /// need loading at the current camera tile, so the next frame keeps scanning
    /// even though the camera hasn't crossed a tile boundary.
    fill_pending: bool,
    /// Last `ClientRuntime::grass_displacer_version` the combined buffer was filtered
    /// against. When it differs, a deployable/building was placed or removed, so the
    /// field is rebuilt to carve grass out of (or restore it under) the new footprints.
    bound_displacer_version: u64,
}

/// Stream detail-grass tiles around the camera. The shader handles the distance
/// fade + wind, so the CPU side just spawns/despawns tile entities.
pub(crate) fn stream_grass_system(
    mut commands: Commands,
    settings: Res<ClientSettings>,
    runtime: Res<ClientRuntime>,
    card_mesh: Res<GrassCardMesh>,
    mut state: ResMut<GrassState>,
) {
    let density = settings.graphics.grass_density;

    // Only stream grass inside a live world; clear the field otherwise.
    let Some(world) = runtime.world.as_ref() else {
        clear_field(&mut commands, &mut state);
        return;
    };
    let Some(blades_per_m2) = blade_density_per_m2(density) else {
        clear_field(&mut commands, &mut state);
        state.bound_density = Some(density);
        return;
    };

    // (Re)build the per-layout instance lists when density changes or on first use,
    // the only place these are (re)allocated. The card mesh itself is the shared
    // density-independent [`GrassCardMesh`] resource built once at startup.
    let density_changed = state.bound_density != Some(density);
    if density_changed || state.layouts.is_empty() {
        state.layouts = (0..GRASS_LAYOUT_COUNT)
            .map(|layout| generate_layout_instances(layout, blades_per_m2))
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

    // Throttle: the loaded tile set is a function of the camera tile, so while
    // the camera stays within one GRASS_TILE_M cell (and no budgeted fill is
    // still draining) nothing can change. Skip the retain + radius rescan
    // entirely on those frames, which is the overwhelming majority (standing,
    // looking around, slow movement). clear_field resets last_cam_tile so a
    // world / density change still forces a fresh scan.
    let cam_tile = (
        (px / GRASS_TILE_M).floor() as i32,
        (pz / GRASS_TILE_M).floor() as i32,
    );
    // A deployable/building placed or removed bumps the displacer version; rebuild the
    // field even when the camera hasn't crossed a tile, so grass re-carves around it.
    let displacer_version = runtime.grass_displacer_version;
    let displacers_changed = state.bound_displacer_version != displacer_version;
    if state.last_cam_tile == Some(cam_tile) && !state.fill_pending && !displacers_changed {
        return;
    }

    // The world seed lets each placed blade pick a biome tint matching the ground.
    let world_seed = runtime.world_map_seed_dims.map(|(seed, _)| seed);

    let blade_mesh = card_mesh.0.clone();

    // Placed-structure footprints overlapping the field, so the combine can drop blades
    // that would poke through a foundation floor, furnace, or sleeping bag. Pre-filtered
    // to the field radius so the per-blade test only sees nearby structures (usually 0).
    let near_displacers: Vec<WorldBlock> = runtime
        .grass_displacers
        .iter()
        .filter(|b| {
            (b.center.x - px).abs() <= GRASS_RADIUS_M + b.half_extents.x
                && (b.center.z - pz).abs() <= GRASS_RADIUS_M + b.half_extents.z
        })
        .copied()
        .collect();

    let GrassState {
        tiles,
        layouts,
        field_entity,
        dirty,
        ..
    } = &mut *state;
    // Re-carve the field this frame if the structure footprints changed (place/remove).
    if displacers_changed {
        *dirty = true;
    }

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
    let (cam_tx, cam_tz) = cam_tile;

    let mut budget = MAX_GRASS_TILE_SPAWNS_PER_FRAME;
    // True if the budget cut the fill short, so tiles still need loading at this
    // camera tile and the next frame must keep scanning (see `fill_pending`).
    let mut hit_budget = false;
    'fill: for tx in (cam_tx - radius_tiles)..=(cam_tx + radius_tiles) {
        for tz in (cam_tz - radius_tiles)..=(cam_tz + radius_tiles) {
            if budget == 0 {
                hit_budget = true;
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
                Some(tile_world_instances(
                    &layouts[layout],
                    cx,
                    cz,
                    yaw,
                    world_seed,
                )),
            );
            *dirty = true;
        }
    }

    // 3. Rebuild the single combined instance buffer when the loaded set changed.
    //    The whole loaded field is one buffer drawn with `NoFrustumCulling`, no
    //    per-region culling: the shader's distance dither thins the far edge, and a
    //    single draw avoids the chunk-by-chunk flicker that per-viewport region
    //    culling caused. At these densities the per-crossing re-upload is a small
    //    memcpy.
    if *dirty {
        *dirty = false;
        let combined: Vec<InstanceData> = tiles
            .values()
            .filter_map(|slot| slot.as_ref())
            .flat_map(|cards| cards.iter().copied())
            // Carve grass out of placed deployables/buildings. `a = [world_x, world_z, ..]`.
            .filter(|inst| {
                near_displacers.is_empty()
                    || !blade_in_displacer(inst.a[0], inst.a[1], &near_displacers)
            })
            .collect();
        update_grass_field(&mut commands, field_entity, &blade_mesh, combined, px, pz);
    }

    // Record what this scan covered so the throttle can skip future frames until
    // the camera crosses a tile (or a budgeted fill still owes tiles here).
    state.last_cam_tile = Some(cam_tile);
    state.fill_pending = hit_budget;
    state.bound_displacer_version = displacer_version;
}

/// Spawn or refresh the single grass-field entity with the combined instance
/// buffer. Despawns it when the field is empty (a 0-length GPU buffer is invalid).
/// `NoFrustumCulling`: blade positions span the whole field but the mesh Aabb is one
/// blade at the origin, and we deliberately draw the whole loaded field (no per-view
/// culling), so Bevy's frustum test must be skipped.
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
                        // Transform (≈ camera) only feeds transparent-sort distance;
                        // blade positions are already world-space.
                        Transform::from_xyz(px, 0.0, pz),
                        Visibility::Visible,
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

/// Card density (cards/m²) for the static menu backdrop. A touch denser than
/// in-world Medium ([`blade_density_per_m2`]) since it's a curated close-up shot.
const MENU_GRASS_BLADES_PER_M2: f32 = 4.0;

/// Spawn a fixed patch of detail grass for the main-menu backdrop, tagged
/// [`WorldGeometry`] so it's torn down with the rest of the backdrop on scene
/// change. Uses the same GPU-instanced tuft pipeline as the in-game grass (same
/// blade mesh, colours, curve, and wind), just as one static buffer, the menu
/// camera barely drifts so streaming isn't needed and the shader's radial fade
/// thins the far edge.
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
            // No world seed for the menu backdrop, so grass stays neutral lush green.
            combined.extend(tile_world_instances(&layouts[layout], cx, cz, yaw, None));
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
/// layouts (the caller updates those).
fn clear_field(commands: &mut Commands, state: &mut GrassState) {
    if let Some(entity) = state.field_entity.take() {
        commands.entity(entity).despawn();
    }
    state.tiles.clear();
    state.dirty = false;
    // Force the next scan to run regardless of camera tile (the field is empty).
    state.last_cam_tile = None;
    state.fill_pending = false;
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

/// Extra clearance (m) around a placed structure's XZ footprint where grass is also
/// removed, so blades don't lean into the edge of a foundation, furnace, or bag.
const GRASS_DISPLACE_MARGIN_M: f32 = 0.2;

/// True if a blade at world `(wx, wz)` sits inside (or within [`GRASS_DISPLACE_MARGIN_M`]
/// of) any placed deployable/building footprint. XZ-only: the structure's height is
/// irrelevant, we just keep grass out of its column so nothing pokes through the floor
/// of a foundation, the centre of a furnace, or a sleeping bag.
fn blade_in_displacer(wx: f32, wz: f32, displacers: &[WorldBlock]) -> bool {
    displacers.iter().any(|b| {
        (wx - b.center.x).abs() <= b.half_extents.x + GRASS_DISPLACE_MARGIN_M
            && (wz - b.center.z).abs() <= b.half_extents.z + GRASS_DISPLACE_MARGIN_M
    })
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
pub(super) fn next_unit(state: &mut u64) -> f32 {
    *state = splitmix64(*state);
    ((*state >> 40) as f32) / ((1u64 << 24) as f32)
}

/// Half-width of one grass card (metres at [`BLADE_REF_HEIGHT`]); per-instance
/// `height_scale` scales the whole card. Cards are wide because each carries a
/// whole textured tuft of blades, not one blade.
pub(super) const CARD_HALF_WIDTH: f32 = 0.24;
/// Quads per card, evenly spaced over a half-turn (2 -> 0/90 degrees, a cross) so
/// the tuft shows a blade silhouette from any horizontal angle. Two (not three)
/// keeps fragment/overdraw cost down; double-sided rendering covers the back.
const CARD_QUADS: u32 = 2;

/// The shared grass-card mesh every tile instances: crossed quads textured with
/// the grass-tuft alpha texture (the blade detail lives in the texture, mipmapped,
/// so far cards fuse into a soft mass instead of aliasing). Baked at
/// [`BLADE_REF_HEIGHT`]; per-instance `height_scale`, `yaw`, and the colour tint
/// are applied in the shader. One card replaces a whole tuft of per-blade geometry,
/// the perf + soft-look win over the old cubic-Bézier blades.
fn build_instanced_blade_mesh() -> Mesh {
    build_grass_card_mesh(BLADE_REF_HEIGHT, CARD_HALF_WIDTH, CARD_QUADS)
}

/// Deterministically scatter one layout's blades evenly into tile-local instance
/// data, centred on the origin spanning `[-GRASS_TILE_M/2, GRASS_TILE_M/2]` so a
/// cardinal rotation about the tile centre maps the square onto itself (no seams).
///
/// Phase 2 carpet model: blades are scattered roughly **uniformly** across the
/// tile (each with a random yaw + height), not gathered into radial rosettes, so
/// a dense field reads as a continuous carpet instead of scattered tuft "stars".
/// A low-frequency fBm only thins the sparsest dips so the carpet still breathes
/// with gentle bald patches rather than being a perfectly even lawn.
fn generate_layout_instances(layout: usize, blades_per_m2: f32) -> Vec<InstanceData> {
    let half = GRASS_TILE_M * 0.5;
    let candidates = (blades_per_m2 * GRASS_TILE_M * GRASS_TILE_M).round() as u32;
    let mut rng = splitmix64(GRASS_LAYOUT_SEED ^ (layout as u64).wrapping_mul(0x100_0001));

    let mut out = Vec::with_capacity(candidates as usize);
    for _ in 0..candidates {
        let bx = next_unit(&mut rng) * GRASS_TILE_M - half;
        let bz = next_unit(&mut rng) * GRASS_TILE_M - half;

        // Density variation: thin only the sparsest noise dips (floor 0.6, so most
        // candidates survive) so coverage stays near-continuous but not uniform.
        let clump = fbm(
            GRASS_CLUMP_SEED ^ layout as u64,
            (bx + half) * 0.12,
            (bz + half) * 0.12,
            1.0,
            3,
        );
        if next_unit(&mut rng) > 0.6 + 0.4 * clump {
            continue;
        }

        // Lean grain: neighbours share a low-frequency lean direction so the field
        // has an organic combed grain instead of every blade pointing randomly,
        // with a wide per-blade jitter so it never reads as a comb.
        let grain = fbm(
            GRASS_LEAN_SEED ^ layout as u64,
            (bx + half) * 0.04,
            (bz + half) * 0.04,
            1.0,
            2,
        );
        let yaw = grain * std::f32::consts::TAU + (next_unit(&mut rng) - 0.5) * 2.4;

        // Organic height: a low-frequency field makes broad tall/short patches so
        // the field undulates like real grass instead of a uniform-height carpet
        // (the single biggest "not a video-game lawn" cue). Per-blade jitter on
        // top. Net ~0.3-1.1 m; the player looks INTO the mass, hiding the ground.
        let hclump = fbm(
            GRASS_HEIGHT_SEED ^ layout as u64,
            (bx + half) * 0.06,
            (bz + half) * 0.06,
            1.0,
            3,
        );
        // Low ground-hugging turf (~0.11-0.48 m after the clump multiplier).
        let height = (0.30 + next_unit(&mut rng) * 0.24) * (0.7 + 0.8 * hclump);
        let height_scale = height / BLADE_REF_HEIGHT;

        // Tonal patches: neighbours share a colour tone (a low-frequency field) so
        // the field mottles into brighter/yellower and darker/cooler patches like a
        // painterly mass, rather than one flat uniform green. Per-blade jitter on top.
        let tone = fbm(
            GRASS_COLOR_SEED ^ layout as u64,
            (bx + half) * 0.10,
            (bz + half) * 0.10,
            1.0,
            3,
        );
        let shade = (0.82 + tone * 0.20) * (0.96 + next_unit(&mut rng) * 0.04);
        // Subtle per-blade hue tint (warm/cool, clump-correlated) baked into the
        // colour tint; the biome grade in `tile_world_instances` multiplies onto it.
        let warm = (tone - 0.45) * 0.55 + (next_unit(&mut rng) - 0.5) * 0.12;
        let tint = [1.0 + warm * 0.10, 1.0 + tone * 0.03, 1.0 - warm * 0.08];
        // Stable per-blade key for biome barrenness thinning (not read by the shader).
        let thin_key = next_unit(&mut rng);
        // Pick one of the tuft-atlas cells at random so the field mixes all six
        // variants (~1/6 each) for natural variety, and the differing tuft
        // heights give size variation for free (no per-blade mesh change). The
        // shader remaps the card UV into this cell.
        let atlas_cell = (next_unit(&mut rng) * GRASS_ATLAS_CELLS as f32)
            .floor()
            .min((GRASS_ATLAS_CELLS - 1) as f32);
        out.push(InstanceData {
            a: [bx, bz, 0.0, height_scale],
            b: [yaw, shade, atlas_cell, thin_key],
            c: [tint[0], tint[1], tint[2], 0.0],
        });
    }
    out
}

/// Fraction of blades thinned out on pure bare rock / ore (where grass barely
/// grows). Scaled by the local rocky+ore biome weight, so a forest/plains tile
/// keeps its full density and rock/ore thin to sparse scrub. High so the rocky
/// and iron (ore) biomes read as nearly bare, only a few scrubby blades.
const GRASS_BIOME_MAX_THIN: f32 = 0.92;

/// Per-biome grass colour tint, multiplied onto the neutral green so grass
/// harmonises with each biome's ground tone (the flat palette in
/// `world::map_texture`): forest stays lush green, plains dries to yellow-green,
/// rocky desaturates toward grey, ore dulls toward brown. `w` is
/// `[forest, rocky, ore, plains]` (renormalised here in case it doesn't sum to 1).
fn biome_grass_tint(w: [f32; 4]) -> [f32; 3] {
    const FOREST: [f32; 3] = [1.00, 1.00, 1.00];
    const ROCKY: [f32; 3] = [0.95, 0.92, 0.85];
    const ORE: [f32; 3] = [1.06, 0.85, 0.62];
    const PLAINS: [f32; 3] = [1.20, 1.04, 0.60];
    let sum = (w[0] + w[1] + w[2] + w[3]).max(1.0e-4);
    let mut out = [0.0f32; 3];
    for ch in 0..3 {
        out[ch] = (w[0] * FOREST[ch] + w[1] * ROCKY[ch] + w[2] * ORE[ch] + w[3] * PLAINS[ch]) / sum;
    }
    out
}

/// Barrenness from biome weights: bare rock and ore carry little grass, so the
/// local rocky+ore weight becomes the thinning factor. `w` is
/// `[forest, rocky, ore, plains]`. Split out so it's testable without the noise field.
fn barrenness_from_weights(w: [f32; 4]) -> f32 {
    (w[1] + w[2]).clamp(0.0, 1.0)
}

/// Transform a layout's tile-local instances into world space for a tile at
/// `(cx, cz)` rotated by `tile_yaw` (one of the four cardinal angles). Rotating
/// the local XZ + folding `tile_yaw` into each blade's spin reproduces the old
/// per-tile `Transform` rotation, but baked into the instance buffer so the
/// shader needs no model matrix.
///
/// With a `world_seed`, bare rock/ore tiles are thinned (grass barely grows there)
/// and each kept blade is graded toward the local biome colour. Without a seed
/// (e.g. the menu backdrop) the field keeps full density and its neutral green.
fn tile_world_instances(
    local: &[InstanceData],
    cx: f32,
    cz: f32,
    tile_yaw: f32,
    world_seed: Option<u64>,
) -> Vec<InstanceData> {
    let (ts, tc) = tile_yaw.sin_cos();

    // Biome varies on a ~600 m scale (`CLASSIFICATION_BASE_FREQUENCY`), so across one
    // 8 m tile the blend weights are effectively constant. Sample the biome ONCE at
    // the tile centre and reuse the thinning cutoff + colour grade for every blade,
    // instead of running four multi-octave fBm channels per blade. The per-blade
    // sample was the dominant cost of the tile-cross rebuild (a ~16 ms spike while
    // walking into fresh terrain, ~2.7k blades × 4 fBm channels per frame); per-tile
    // is ~230× fewer noise samples for a visually identical result. The per-blade
    // stable key still decides each blade individually against the cutoff, so the
    // thinning stays a smooth random scatter rather than a hard per-tile on/off.
    let biome = world_seed.map(|seed| {
        let weights = biome_blend_weights(ClassificationChannels::sample_at(seed, cx, cz));
        let cut = barrenness_from_weights(weights) * GRASS_BIOME_MAX_THIN;
        (cut, biome_grass_tint(weights))
    });

    local
        .iter()
        .filter_map(|inst| {
            let lx = inst.a[0];
            let lz = inst.a[1];
            let wx = cx + tc * lx + ts * lz;
            let wz = cz - ts * lx + tc * lz;
            let a = [wx, wz, inst.a[2], inst.a[3]];
            let b = [inst.b[0] + tile_yaw, inst.b[1], inst.b[2], inst.b[3]];
            let Some((cut, t)) = biome else {
                return Some(InstanceData { a, b, c: inst.c });
            };
            // Drop the blade if its stable key falls under the tile's barren cut.
            if inst.b[3] < cut {
                return None;
            }
            Some(InstanceData {
                a,
                b,
                c: [inst.c[0] * t[0], inst.c[1] * t[1], inst.c[2] * t[2], 0.0],
            })
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
        assert!(blade_density_per_m2(GrassDensity::Off).is_none());
        assert!(blade_density_per_m2(GrassDensity::Low).is_some());
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
        let world = tile_world_instances(&local, 100.0, -50.0, std::f32::consts::FRAC_PI_2, None);
        assert_eq!(local.len(), world.len());
        // Local origin-ish blade maps near the tile centre, and the tile yaw is
        // folded into each blade's spin.
        assert!((world[0].b[0] - (local[0].b[0] + std::f32::consts::FRAC_PI_2)).abs() < 1e-4);
    }

    #[test]
    fn barrenness_thins_rock_and_ore_not_forest_or_plains() {
        // Forest and plains carry grass (no thinning); bare rock and ore are barren.
        assert_eq!(barrenness_from_weights([1.0, 0.0, 0.0, 0.0]), 0.0);
        assert_eq!(barrenness_from_weights([0.0, 0.0, 0.0, 1.0]), 0.0);
        assert_eq!(barrenness_from_weights([0.0, 1.0, 0.0, 0.0]), 1.0);
        assert_eq!(barrenness_from_weights([0.0, 0.0, 1.0, 0.0]), 1.0);
        // A rock+ore mix is fully barren; half-plains/half-rock is partial.
        assert_eq!(barrenness_from_weights([0.0, 0.5, 0.5, 0.0]), 1.0);
        assert!((barrenness_from_weights([0.0, 0.5, 0.0, 0.5]) - 0.5).abs() < 1e-6);
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
