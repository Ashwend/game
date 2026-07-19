use bevy::{
    asset::RenderAssetUsages,
    audio::SpatialListener,
    camera::{ClearColorConfig, Hdr, visibility::RenderLayers},
    core_pipeline::tonemapping::Tonemapping,
    gltf::GltfAssetLabel,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    light::{Atmosphere, AtmosphereEnvironmentMapLight, atmosphere::ScatteringMedium},
    pbr::AtmosphereSettings,
    post_process::dof::{DepthOfField, DepthOfFieldMode},
    prelude::*,
    render::{
        render_resource::{Extent3d, TextureDimension, TextureFormat},
        view::NoIndirectDrawing,
    },
};

use bevy_egui::PrimaryEguiContext;

use super::mesh::builder::build_hay_tuft_mesh;
use super::terrain::build_mip_chain;
use super::toon::{ToonMaterial, ToonViewmodelMaterial};
use super::{
    components::{MainCamera, VIEWMODEL_RENDER_LAYER, ViewmodelCamera},
    deployable_assets::DeployableVisualAssets,
    materials::{hay_tall_grass_material, tree_texture_sampler},
    mesh::{
        ORE_NODE_STAGE_COUNT, PlayerRigMeshes, build_player_rig_meshes, door_ghost_mesh,
        impact_stone_shard_mesh, impact_wood_chip_mesh, low_poly_bag_mesh,
        low_poly_birch_tree_large_lod_mesh, low_poly_birch_tree_medium_lod_mesh,
        low_poly_birch_tree_small_lod_mesh, low_poly_branch_pile_mesh,
        low_poly_pine_tree_large_lod_mesh, low_poly_pine_tree_medium_lod_mesh,
        low_poly_pine_tree_small_lod_mesh, low_poly_surface_stone_mesh,
    },
    meteor_sky::setup_meteor_sky,
    sky::{initial_distance_fog, setup_sky},
};
use crate::app::embedded_assets::embedded_bytes;

use crate::app::{EYE_HEIGHT, PLAYER_VISUAL_CENTER_Y, embedded_asset_path};

/// Strength of the image-based ambient/reflection light generated from the
/// procedural sky. The sun is kept at a daylight-calibrated illuminance (see
/// `SUN_PEAK_ILLUMINANCE` in `sky.rs`) with the renderer's default exposure.
/// Trimmed below the physical `1.0` so the sky doesn't flood every surface with
/// fill light: that flat, shadowless fill was a big part of the washed-out,
/// low-contrast "dreamy" read. Lower still for moodier, deeper shadows.
pub(crate) const ATMOSPHERE_AMBIENT_INTENSITY: f32 = 0.70;

/// Cubemap resolution (per face) of the atmosphere environment map used for
/// IBL. Bevy's default is `512`, but that cubemap is **refiltered every frame**
/// (no skip-if-unchanged gating in Bevy 0.18) and dominated our GPU cost
/// (500→70 fps). Our materials are almost all matte and the shiniest (iron ore)
/// is still roughness 0.78, no mirrors, while diffuse irradiance needs almost
/// no resolution. So `64` is visually indistinguishable here yet ~64× cheaper
/// to filter than the default. Raise it if a glossier material is ever added.
pub(crate) const ATMOSPHERE_ENV_MAP_SIZE: u32 = 64;

pub(crate) const WORLD_COLOR: Color = Color::srgb(0.18, 0.34, 0.22);
pub(crate) const DROPPED_BAG_COLOR: Color = Color::srgb(0.42, 0.31, 0.18);
pub(crate) const HELD_BAG_COLOR: Color = Color::srgb(0.50, 0.38, 0.24);
pub(crate) const VERTEX_MATERIAL_COLOR: Color = Color::WHITE;

#[derive(Resource, Clone)]
pub(crate) struct PlayerVisualAssets {
    /// Per-part meshes for the rigged remote body. The reconciler in
    /// `app::systems::players` spawns one child entity per part and clones the
    /// matching handle.
    pub(crate) rig: PlayerRigMeshes,
    /// Shared base-white material; the per-part vertex colours do the look.
    /// Cloned per corpse on death so a fade doesn't drag every live player
    /// along (see `tick_dying_players_system`).
    pub(crate) remote_material: Handle<StandardMaterial>,
}

#[derive(Resource, Clone)]
pub(crate) struct ItemVisualAssets {
    pub(crate) dropped_mesh: Handle<Mesh>,
    /// The one procedural cuboid the bag silhouette (raw materials +
    /// deployables-in-hand) renders as. The authored tool/hammer/plan glbs load
    /// through the [`crate::items::HeldMesh`] visual table into
    /// `HeldItemVisuals`, not fields here.
    pub(crate) held_bag_mesh: Handle<Mesh>,
    /// Shared cel [`ToonMaterial`]s for the tool layers, resolved from a
    /// [`crate::items::HeldMeshMaterial`] family. The tools joined the cel/anime
    /// family, so each layer is lit by the same PBR-then-posterise path as the ore
    /// and deployables. Wood carries the haft plus its twine bindings; Stone is
    /// knapped stone; Iron is forged steel plus the hammer bands; Parchment is the
    /// building-plan paper. Per-item colour comes from the glb COLOR_0 (warm wood,
    /// tan twine, grey stone, cool steel); the light neutral-grain textures only
    /// add surface detail (`detail * COLOR_0`). Sources: `assets/textures/tools/*`.
    pub(crate) tool_wood_material: Handle<ToonMaterial>,
    pub(crate) tool_stone_material: Handle<ToonMaterial>,
    pub(crate) tool_iron_material: Handle<ToonMaterial>,
    /// Camera-relative variants of the three tool materials, used only for the
    /// FIRST-PERSON held viewmodel (a camera-child). They light the cel bands from
    /// a fixed view-space key light so the bands don't swim as the camera turns;
    /// the world-space `*_material` above stays on the third-person tool, which is
    /// in world space on a remote player's hand and should be lit like the scene.
    pub(crate) tool_wood_vm_material: Handle<ToonViewmodelMaterial>,
    pub(crate) tool_stone_vm_material: Handle<ToonViewmodelMaterial>,
    pub(crate) tool_iron_vm_material: Handle<ToonViewmodelMaterial>,
    /// Parchment material for the rolled building-plan scroll (world + viewmodel
    /// variants, same as the tools). Its twine ties reuse the wood material with a
    /// brown COLOR_0.
    pub(crate) tool_parchment_material: Handle<ToonMaterial>,
    pub(crate) tool_parchment_vm_material: Handle<ToonViewmodelMaterial>,
    /// Woven-cloth cel material for the explosive charges (world + viewmodel):
    /// the powder bomb's wrap and the satchel's pack body bind this, and the
    /// satchel's leather strap reuses it too (both ride the shared `cloth.png`
    /// tool tile; each glb's COLOR_0 carries the fabric colour vs the tan leather,
    /// the same `detail * COLOR_0` cel path the tools use).
    pub(crate) tool_cloth_material: Handle<ToonMaterial>,
    pub(crate) tool_cloth_vm_material: Handle<ToonViewmodelMaterial>,
    /// Pale bowstring / crossbow-string cord cel material (world + viewmodel). The
    /// bow and crossbow string legs bind this; each glb's COLOR_0 carries the
    /// pale-tan cord colour, and the neutral-grain tile only adds a faint twist of
    /// surface detail (`detail * COLOR_0`, the same cel path the tools use). Reuses
    /// the light neutral `parchment` tile (there is no dedicated cord texture; the
    /// cord is slim enough that its grain barely reads, only its colour does).
    pub(crate) tool_cord_material: Handle<ToonMaterial>,
    pub(crate) tool_cord_vm_material: Handle<ToonViewmodelMaterial>,
    pub(crate) dropped_material: Handle<StandardMaterial>,
    /// Flat `StandardMaterial` the bag silhouette binds (resolved from the
    /// `BagStandard` family; no cel/viewmodel variant).
    pub(crate) held_bag_material: Handle<StandardMaterial>,
}

#[derive(Resource, Clone)]
pub(crate) struct ResourceVisualAssets {
    /// Ore/vein meshes indexed by visual depletion stage (0 = untouched,
    /// see `ORE_NODE_STAGE_COUNT`). The mirror entity's mesh handle is
    /// swapped between these as the replicated storage crosses the stage
    /// thresholds.
    pub(crate) coal_node_meshes: [Handle<Mesh>; ORE_NODE_STAGE_COUNT],
    pub(crate) iron_node_meshes: [Handle<Mesh>; ORE_NODE_STAGE_COUNT],
    pub(crate) sulfur_node_meshes: [Handle<Mesh>; ORE_NODE_STAGE_COUNT],
    pub(crate) stone_vein_meshes: [Handle<Mesh>; ORE_NODE_STAGE_COUNT],
    /// Meteorite (rare node): a squat scorched slag mound studded with pale
    /// raw-alloy nuggets, per depletion stage. Distinct silhouette, but the
    /// same shared `ore_toon_material` as every other ore (nothing glows).
    pub(crate) meteorite_node_meshes: [Handle<Mesh>; ORE_NODE_STAGE_COUNT],
    /// Full-detail live trees, authored as Blender glbs (`art/trees/*`): each
    /// loads two primitives, a textured bark TRUNK (opaque, `*_bark_material`)
    /// and an alpha-masked needle/leaf CANOPY (`*_foliage_material`), spawned as
    /// trunk parent + foliage child (see `resource_nodes::spawn`). Switched out
    /// for the cheap `*_lod_mesh` past the LOD distance.
    pub(crate) pine_tree_small_trunk_mesh: Handle<Mesh>,
    pub(crate) pine_tree_small_foliage_mesh: Handle<Mesh>,
    pub(crate) pine_tree_medium_trunk_mesh: Handle<Mesh>,
    pub(crate) pine_tree_medium_foliage_mesh: Handle<Mesh>,
    pub(crate) pine_tree_large_trunk_mesh: Handle<Mesh>,
    pub(crate) pine_tree_large_foliage_mesh: Handle<Mesh>,
    pub(crate) birch_tree_small_trunk_mesh: Handle<Mesh>,
    pub(crate) birch_tree_small_foliage_mesh: Handle<Mesh>,
    pub(crate) birch_tree_medium_trunk_mesh: Handle<Mesh>,
    pub(crate) birch_tree_medium_foliage_mesh: Handle<Mesh>,
    pub(crate) birch_tree_large_trunk_mesh: Handle<Mesh>,
    pub(crate) birch_tree_large_foliage_mesh: Handle<Mesh>,
    /// Low-poly distance LOD variants of the trees, swapped in past the LOD
    /// distance via `VisibilityRange` hard switch (see the resource-node spawn).
    pub(crate) pine_tree_small_lod_mesh: Handle<Mesh>,
    pub(crate) pine_tree_medium_lod_mesh: Handle<Mesh>,
    pub(crate) pine_tree_large_lod_mesh: Handle<Mesh>,
    pub(crate) birch_tree_small_lod_mesh: Handle<Mesh>,
    pub(crate) birch_tree_medium_lod_mesh: Handle<Mesh>,
    pub(crate) birch_tree_large_lod_mesh: Handle<Mesh>,
    /// Bare dead-tree snags by size, scattered in non-forest biomes (chosen
    /// client-side from the seed at spawn). No canopy, so they're size-only (a
    /// dead pine and dead birch of the same size share a mesh) and carry no
    /// separate LOD, they're already low-poly.
    pub(crate) dead_tree_small_mesh: Handle<Mesh>,
    pub(crate) dead_tree_medium_mesh: Handle<Mesh>,
    pub(crate) dead_tree_large_mesh: Handle<Mesh>,
    pub(crate) surface_stone_mesh: Handle<Mesh>,
    pub(crate) branch_pile_mesh: Handle<Mesh>,
    pub(crate) hay_grass_mesh: Handle<Mesh>,
    /// Per-type cel-shaded ore materials. Each ore glb carries a baked albedo
    /// (`assets/textures/ore/<type>.png`, rebaked from the image-to-3D
    /// output's texture onto the low-poly UVs by
    /// `art/ore/rework/build_nodes.py`), so the mineral identity lives in the
    /// texture; the glb COLOR_0 is white. Same toon ramp/params across all
    /// five; the three depletion stages of a type share its texture because
    /// the stage meshes are scaled copies with identical UVs.
    pub(crate) stone_vein_material: Handle<ToonMaterial>,
    pub(crate) iron_node_material: Handle<ToonMaterial>,
    pub(crate) coal_node_material: Handle<ToonMaterial>,
    pub(crate) sulfur_node_material: Handle<ToonMaterial>,
    pub(crate) meteorite_node_material: Handle<ToonMaterial>,
    pub(crate) vertex_material: Handle<StandardMaterial>,
    /// Cel-shaded ([`ToonMaterial`]) alpha-masked cards for the harvestable hay
    /// tuft: the shared grass-tuft texture, lit by the same PBR-then-posterise path
    /// as the detail grass and trees (see [`build_hay_tuft_mesh`]). Three variants
    /// (each a different seed-headed tuft card); a hay node picks one by `id % 3`
    /// so the harvestable plants vary.
    pub(crate) hay_grass_materials: [Handle<ToonMaterial>; 3],
    /// Cel-shaded ([`ToonMaterial`]) tree bark + canopy foliage, one per species
    /// per surface, shared by every instance so the forest batches by
    /// mesh+material. Built from the embedded `textures/trees/*.png` painted
    /// detail (see `load_tree_texture`); the glb COLOR_0 tints them per layer, the
    /// same `texture * COLOR_0` cel path the ore nodes use. The canopy is solid
    /// faceted geometry (not alpha cards), so these are all opaque.
    pub(crate) pine_bark_material: Handle<ToonMaterial>,
    pub(crate) pine_foliage_material: Handle<ToonMaterial>,
    pub(crate) birch_bark_material: Handle<ToonMaterial>,
    pub(crate) birch_foliage_material: Handle<ToonMaterial>,
    /// Cel-shaded weathered bark for the dead-snag trunks: the pine bark detail
    /// over the snag glb's cool-grey COLOR_0 so a leafless tree reads as "dead",
    /// not just a live trunk without leaves.
    pub(crate) dead_bark_material: Handle<ToonMaterial>,
}

/// Mesh + material handles for the furnace fire visuals (the flickering flame
/// tongue and its rising sparks). Built once in `setup_scene`; consumed by the
/// furnace-fire systems in `app::systems::furnace_fire`.
#[derive(Resource, Clone)]
pub(crate) struct FurnaceFireAssets {
    pub(crate) flame_mesh: Handle<Mesh>,
    pub(crate) flame_material: Handle<StandardMaterial>,
    pub(crate) spark_mesh: Handle<Mesh>,
    pub(crate) spark_material: Handle<StandardMaterial>,
}

/// Mesh + material handles for the torch fire visuals. Built once in
/// `setup_scene`; consumed by the torch-fire systems in
/// `app::systems::torch_fire`. The flame is a sparse particle puff up close;
/// the billboard is a single camera-facing emissive quad shown in its place
/// at distance (the cheap LOD that replaces the particles far away).
#[derive(Resource, Clone)]
pub(crate) struct TorchFireAssets {
    pub(crate) flame_mesh: Handle<Mesh>,
    pub(crate) flame_material: Handle<StandardMaterial>,
    pub(crate) billboard_mesh: Handle<Mesh>,
    pub(crate) billboard_material: Handle<StandardMaterial>,
}

/// Mesh + material handles for the meteor shower's shed particles. Built once
/// in `setup_scene`; consumed by the meteor systems in `app::scene::meteor_sky`. The
/// torch/furnace flame templates were too dim for a fireball reading against the
/// bright day sky, so the meteor gets its own dedicated set: a bright additive
/// spark (furnace-spark HDR ratio, proven to hold orange in daylight) and a
/// warm-dark blended smoke puff shed under the fire trail.
#[derive(Resource, Clone)]
pub(crate) struct MeteorEmberAssets {
    pub(crate) ember_mesh: Handle<Mesh>,
    pub(crate) ember_material: Handle<StandardMaterial>,
    pub(crate) smoke_mesh: Handle<Mesh>,
    pub(crate) smoke_material: Handle<StandardMaterial>,
    /// Unit sphere for the one-shot airburst flash (the material is created
    /// per burst so its fade-out never touches a shared handle).
    pub(crate) flash_mesh: Handle<Mesh>,
}

#[derive(Resource, Clone)]
pub(crate) struct ImpactEffectAssets {
    pub(crate) wood_chip_mesh: Handle<Mesh>,
    pub(crate) stone_shard_mesh: Handle<Mesh>,
    /// Small round droplet for the PvP blood spray (a low-poly sphere, so it
    /// reads as a blob rather than the angular rock-shard mesh).
    pub(crate) blood_droplet_mesh: Handle<Mesh>,
    /// Flat unit disc laid on the ground for the lingering blood pool.
    pub(crate) blood_splatter_mesh: Handle<Mesh>,
    pub(crate) wood_chip_material: Handle<StandardMaterial>,
    pub(crate) stone_shard_material: Handle<StandardMaterial>,
    /// Green-tinted material used for the `GrassBlades` particle burst.
    /// The mesh is reused from the stone shard so we don't pay for a
    /// second tiny mesh, the base-colour shift is enough to read as
    /// grass debris.
    pub(crate) grass_blade_material: Handle<StandardMaterial>,
    /// Deep-red material for the `FleshHit` (PvP) blood spray. Reuses the stone
    /// shard mesh like the grass burst; the red base colour multiplies through
    /// the shard's vertex colours so the chips read as blood droplets.
    pub(crate) blood_material: Handle<StandardMaterial>,
}

/// Startup system wiring the whole client scene: the shared defaults, the
/// cameras, the sky + meteor rigs, and every visual-asset family. Each section
/// lives in a focused helper below; the call order preserves the original
/// top-to-bottom wiring (spawn order can be load-bearing for render layers, so
/// do not reorder the calls).
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn setup_scene(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut toon_materials: ResMut<Assets<ToonMaterial>>,
    mut toon_viewmodel_materials: ResMut<Assets<ToonViewmodelMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut scattering_media: ResMut<Assets<ScatteringMedium>>,
) {
    let toon_no_glow_tex = insert_shared_defaults(&mut commands, &mut images);
    spawn_cameras(&mut commands, &mut scattering_media);
    setup_sky(&mut commands, &mut meshes, &mut materials);
    setup_meteor_sky(&mut commands, &mut meshes, &mut materials);

    commands.insert_resource(super::world::WorldSceneState::default());
    insert_player_visuals(&mut commands, &mut meshes, &mut materials);
    insert_held_item_visuals(
        &mut commands,
        &asset_server,
        &mut meshes,
        &mut materials,
        &mut toon_materials,
        &mut toon_viewmodel_materials,
        &mut images,
        &toon_no_glow_tex,
    );
    insert_armor_visuals(&mut commands, &asset_server, &mut materials, &mut images);
    insert_resource_node_visuals(
        &mut commands,
        &asset_server,
        &mut meshes,
        &mut materials,
        &mut toon_materials,
        &mut images,
        &toon_no_glow_tex,
    );
    insert_deployable_visuals(
        &mut commands,
        &asset_server,
        &mut meshes,
        &mut materials,
        &mut toon_materials,
        &mut images,
        &toon_no_glow_tex,
    );
    insert_impact_effect_assets(&mut commands, &mut meshes, &mut materials);
    insert_explosion_effect_assets(&mut commands, &mut meshes, &mut materials);
    insert_fire_particle_assets(&mut commands, &mut meshes, &mut materials);
}

/// Cross-family shared setup: the per-biome terrain textures, the 1x1 white
/// "no glow" emissive mask every cel material binds (returned so the material
/// builders below can clone it), and the first-frame ambient default.
fn insert_shared_defaults(commands: &mut Commands, images: &mut Assets<Image>) -> Handle<Image> {
    // The four shared per-biome ground textures for the textured terrain floor.
    // Decoded + mip-chained once here; each world bakes only its own small
    // biome-weight raster when its ground spawns (see `super::world::spawn_world_geometry`).
    commands.insert_resource(super::TerrainTextureAssets::load(images));

    // 1x1 white "no glow" mask, the inert emissive default every cel prop
    // binds (see `ToonMaterial::emissive`). Created up front so every material
    // builder in the helpers below can clone it. A white mask + a zero
    // `emissive` tint makes the shader's emissive term evaluate to zero. No
    // material binds a real glow any more (the old meteorite crystal emissive
    // is gone; nothing in this world is magic), so the term is currently a
    // no-op kept for the material API.
    let toon_no_glow_tex = images.add(Image::new_fill(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[255, 255, 255, 255],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    ));
    // Ambient and clear color are now driven by the day/night cycle in
    // `sky::update_sky_system`. We still insert defaults here so the
    // very first frame (before the system runs) has sensible values
    // rather than the engine defaults.
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.72, 0.78, 0.86),
        brightness: 90.0,
        ..default()
    });

    toon_no_glow_tex
}

/// Spawn the world camera (HDR + atmosphere settings + fog + spatial listener),
/// the standalone `Atmosphere` entity, and the dedicated first-person viewmodel
/// camera that renders the held-item layer over the finished scene.
fn spawn_cameras(commands: &mut Commands, scattering_media: &mut Assets<ScatteringMedium>) {
    let main_camera = commands
        .spawn((
            Name::new("Camera"),
            MainCamera,
            // Own the single primary Egui context explicitly (auto-creation is
            // disabled in `app.rs`). All UI runs in `EguiPrimaryContextPass`, so the
            // context must live on the world camera, not the sibling viewmodel
            // camera, or UI keyboard input breaks (open chat but can't type). Pulls
            // in `EguiContext` + the multipass schedule via its required components.
            PrimaryEguiContext,
            Camera3d::default(),
            // HDR is a permanent baseline: bloom needs it, and the procedural
            // atmosphere sky (Phase 2) requires it. It only changes the
            // intermediate render texture, not the swapchain. Tonemapping is set
            // explicitly to the filmic TonyMcMapface curve, which desaturates the
            // brightest values so bloom + a hot sun disc read as glow rather than
            // a clipped white blob. Bloom itself is owned by
            // `apply_graphics_settings_system` so it tracks the Graphics tab.
            Hdr,
            // Opt this camera out of GPU-driven indirect batching. With it on, the
            // binned opaque phase intermittently dropped whole batches (regions of
            // trees/ore vanishing until you moved) once a second pipeline, the
            // custom grass material, and earlier `VisibilityRange` entities, shared
            // the phase. Direct (non-indirect) drawing is stable here; with ~1k
            // visible meshes the CPU draw-submission cost is negligible, and macOS
            // Metal has limited multi-draw-indirect support anyway.
            NoIndirectDrawing,
            // AgX: a flatter, more desaturated/pastel filmic curve than TonyMcMapface,
            // which reads softer and more painterly (closer to the stylized-grass
            // reference). Scene-wide; verified against the daytime/dusk scene.
            Tonemapping::AgX,
            // Procedural physically-based sky. In Bevy 0.19 the `Atmosphere` itself
            // lives on its own entity (spawned just below); a 3D camera opts in
            // simply by carrying `AtmosphereSettings` (the renderer picks the nearest
            // atmosphere), plus the `AtmosphereEnvironmentMapLight` that turns the sky
            // into image-based ambient. The atmosphere reads the sun `DirectionalLight`
            // to place the sun disc and tint sunlight through the air, and renders the
            // sky behind all geometry, so the old hand-authored `ClearColor` sky is
            // retired. Grouped into a nested sub-bundle so the camera's component tuple
            // stays under Bevy's 15-element bundle arity limit.
            (
                // The atmosphere recomputes its LUTs every frame (no skip-if-unchanged
                // gating), so these are a per-frame GPU cost. We trim them
                // for performance, favouring sample-count cuts (slightly noisier
                // integration, ~imperceptible) over resolution cuts (which band). The
                // transmittance/multiscattering *resolutions* stay at default to keep
                // sky-colour fidelity. Defaults shown in comments for reference.
                AtmosphereSettings {
                    transmittance_lut_samples: 24,           // default 40
                    multiscattering_lut_samples: 12,         // default 20
                    sky_view_lut_size: UVec2::new(256, 128), // default 400×200
                    sky_view_lut_samples: 12,                // default 16
                    aerial_view_lut_samples: 6,              // default 10
                    ..default()
                },
                // Image-based ambient + reflections generated from the atmosphere.
                // This is the "free IBL" that lifts every PBR surface; it supplies the
                // daytime ambient term (the sky's `GlobalAmbientLight` floor fades to
                // zero by day, see `sky.rs`). Strength via `ATMOSPHERE_AMBIENT_INTENSITY`.
                AtmosphereEnvironmentMapLight {
                    intensity: ATMOSPHERE_AMBIENT_INTENSITY,
                    // Small cubemap, refiltered every frame, so this is the main GPU
                    // cost lever. See `ATMOSPHERE_ENV_MAP_SIZE`.
                    size: UVec2::splat(ATMOSPHERE_ENV_MAP_SIZE),
                    ..default()
                },
            ),
            Projection::from(PerspectiveProjection {
                fov: 65.0_f32.to_radians(),
                // The far plane sits *well past* the distance at which the squared
                // distance fog goes fully opaque (~260 m for the 190 m daytime
                // visibility, tighter at dusk/night), so the ground plane and any
                // far geometry dissolve completely into the fog before the far
                // plane would clip them. The old 160 m far plane sat *inside* that
                // fade band: it hard-cut a still-faintly-visible ring of floor,
                // which flickered as the camera rotated and let the un-fogged
                // atmosphere sky (and the setting sun) show through the cut. The
                // perimeter walls (>=480 m even on a Small map) stay beyond the far
                // plane and are fully fogged, so they still never draw. Reverse-Z
                // keeps depth precision fine at this range.
                near: 0.05,
                far: 300.0,
                ..default()
            }),
            Msaa::Off,
            menu_backdrop_depth_of_field(),
            // ~17cm between ears, keeps L/R panning natural for nearby spatial
            // sound sources. Bevy's default (4.0) is tuned for huge open worlds
            // and exaggerates panning at first-person ranges.
            SpatialListener::new(0.17),
            // Atmospheric haze: faded by the day/night system per-frame, but
            // present from frame zero so far geometry never pops into a
            // colourless void on the first render.
            initial_distance_fog(),
            Transform::from_xyz(0.0, EYE_HEIGHT, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();

    // The procedural sky's `Atmosphere` is a standalone entity in Bevy 0.19 (it was
    // a camera component in 0.18). Its `GlobalTransform` marks the planet centre; the
    // component's on-add hook auto-places an untouched transform `inner_radius` below
    // the origin, so the scene sits on the planet surface with no manual transform
    // needed. The camera opts in via the `AtmosphereSettings` in its bundle above.
    // Earth scattering medium at 256x256 LUT resolution (Bevy's atmosphere-example
    // default; `ScatteringMedium::default()` no longer exists in 0.19).
    commands.spawn((
        Name::new("Atmosphere"),
        Atmosphere::earth(scattering_media.add(ScatteringMedium::earth(256, 256))),
    ));

    // Dedicated first-person viewmodel camera. It is a child of the world camera
    // (so it shares the eye transform every frame for free) and renders ONLY the
    // held-item layer over the finished scene, in its own pass with a fresh,
    // cleared depth buffer. That depth isolation is the whole point: the in-hand
    // tool no longer depth-tests against the world, so it stops being sliced by /
    // clipping into a wall, ore boulder, or peer the player stands close to. Same
    // FOV + HDR + AgX tonemap as the world camera so the tool's proportions and
    // grading match; no Atmosphere/IBL of its own (that would double the per-frame
    // sky-cubemap refilter, the scene's single biggest GPU cost) so the tool's
    // brightness rides the `ToonViewmodelMaterial` probe's sun + ambient term,
    // which is built to degrade gracefully without the sky IBL.
    commands.spawn((
        Name::new("Viewmodel Camera"),
        ViewmodelCamera,
        ChildOf(main_camera),
        Camera3d::default(),
        Camera {
            // Render after the world camera (order 0) and composite on top without
            // clearing colour, so the tool draws over the finished frame.
            order: 1,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        Hdr,
        Tonemapping::AgX,
        NoIndirectDrawing,
        Projection::from(PerspectiveProjection {
            // The base FOV constant is shared with the ranged-draw pinch sync
            // (`sync_viewmodel_fov_system`), which is the ONLY thing that ever
            // rewrites this projection: it pinches proportionally while a bow
            // draw is held and restores exactly this value otherwise.
            fov: crate::app::systems::VIEWMODEL_BASE_FOV_DEG.to_radians(),
            // A tight, fully self-contained depth range for the tool. The near
            // plane sits right at the lens so the tool never near-clips even on a
            // hard swing, and the short far plane keeps depth precision dense.
            near: 0.01,
            far: 5.0,
            ..default()
        }),
        Msaa::Off,
        // Sees ONLY the held-item layer; the world stays on the default layer 0.
        RenderLayers::layer(VIEWMODEL_RENDER_LAYER),
        Transform::IDENTITY,
    ));
}

/// Shared rig meshes + base-white material for the rigged remote player bodies.
fn insert_player_visuals(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    commands.insert_resource(PlayerVisualAssets {
        rig: build_player_rig_meshes(meshes),
        remote_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.92,
            reflectance: 0.2,
            ..default()
        }),
    });
}

/// Held-item visuals: the shared tool/cloth/cord cel materials (world +
/// viewmodel variants), the bag silhouette, and the precomputed
/// `HeldItemVisuals` layer stacks.
#[expect(clippy::too_many_arguments, reason = "split-out system helper")]
fn insert_held_item_visuals(
    commands: &mut Commands,
    asset_server: &AssetServer,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    toon_materials: &mut Assets<ToonMaterial>,
    toon_viewmodel_materials: &mut Assets<ToonViewmodelMaterial>,
    images: &mut Assets<Image>,
    toon_no_glow_tex: &Handle<Image>,
) {
    // Held items (the tools, the construction hammer, the building-plan scroll)
    // each ship as an authored Blender glb matching their inventory icon. Their
    // glb paths and per-primitive material families now live in the declarative
    // `HeldMesh::visual` table (src/items/visual.rs); `build_held_item_visuals`
    // (below) folds that table into the `HeldItemVisuals` lookup, loading each
    // glb primitive and resolving each layer's material family to the shared
    // handles built here. So a new held item is one table row plus its glb, not
    // per-item fields + loads + a match arm across three files.
    // Tool cel textures: light neutral-grain detail (wood grain / knapped stone /
    // hammered steel) that the glb COLOR_0 tints, the same `detail * COLOR_0`
    // trick as the ore + deployables. Decoded sRGB with a CPU mip chain +
    // repeat/aniso sampler. Source: `art/tools/make_tool_textures.py`.
    let mut load_tool_texture = |name: &str| -> Handle<Image> {
        let rel = format!("textures/tools/{name}.png");
        let bytes =
            embedded_bytes(&rel).unwrap_or_else(|| panic!("embedded tool texture missing: {rel}"));
        let mut image = Image::from_buffer(
            bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::Descriptor(tree_texture_sampler()),
            RenderAssetUsages::RENDER_WORLD,
        )
        .unwrap_or_else(|err| panic!("decode tool texture {rel}: {err:?}"));
        build_mip_chain(&mut image);
        images.add(image)
    };
    // Tool heads + haft are flat-faceted, so they run the punchy deployable cel
    // params (3 bands, full-strength + slightly wider ink edge so every facet and
    // bevel draws an outline). `tex_scale` is unused (the glbs carry real UVs).
    let tool_params = Vec4::new(3.0, 0.0, 1.0, 1.4);
    let tool_toon = |tex: Handle<Image>| ToonMaterial {
        detail: tex,
        params: tool_params,
        tex_scale: 1.0,
        fade: 1.0,
        dev_flags: 0,
        emissive_tex: toon_no_glow_tex.clone(),
        emissive: Vec4::ZERO,
    };
    // Camera-relative variant for the first-person held viewmodel (same texture +
    // params; only the shader's light frame differs, see `ToonViewmodelMaterial`).
    let tool_vm = |tex: Handle<Image>| ToonViewmodelMaterial {
        detail: tex,
        params: tool_params,
        tex_scale: 1.0,
        fade: 1.0,
        dev_flags: 0,
    };
    let wood_tex = load_tool_texture("wood");
    let stone_tex = load_tool_texture("stone");
    let iron_tex = load_tool_texture("iron");
    let parchment_tex = load_tool_texture("parchment");
    let tool_wood_material = toon_materials.add(tool_toon(wood_tex.clone()));
    let tool_stone_material = toon_materials.add(tool_toon(stone_tex.clone()));
    let tool_iron_material = toon_materials.add(tool_toon(iron_tex.clone()));
    let tool_parchment_material = toon_materials.add(tool_toon(parchment_tex.clone()));
    let tool_wood_vm_material = toon_viewmodel_materials.add(tool_vm(wood_tex));
    let tool_stone_vm_material = toon_viewmodel_materials.add(tool_vm(stone_tex));
    let tool_iron_vm_material = toon_viewmodel_materials.add(tool_vm(iron_tex));
    let tool_parchment_vm_material = toon_viewmodel_materials.add(tool_vm(parchment_tex.clone()));
    // Explosive-charge cloth family: the powder bomb's wrap, the satchel's pack
    // body, and the satchel's leather strap all bind this one cloth-tile cel
    // material (each glb's COLOR_0 gives it the fabric vs tan-leather colour).
    let cloth_tex = load_tool_texture("cloth");
    let tool_cloth_material = toon_materials.add(tool_toon(cloth_tex.clone()));
    let tool_cloth_vm_material = toon_viewmodel_materials.add(tool_vm(cloth_tex));
    // Bowstring / crossbow-string cord: a slim pale cord whose COLOR_0 carries the
    // colour. Reuses the neutral-grain parchment tile (no dedicated cord texture);
    // the string is thin enough that only its COLOR_0 reads.
    let tool_cord_material = toon_materials.add(tool_toon(parchment_tex.clone()));
    let tool_cord_vm_material = toon_viewmodel_materials.add(tool_vm(parchment_tex.clone()));
    let item_visual_assets = ItemVisualAssets {
        dropped_mesh: meshes.add(low_poly_bag_mesh()),
        held_bag_mesh: meshes.add(Cuboid::new(0.26, 0.22, 0.34)),
        tool_wood_material,
        tool_stone_material,
        tool_iron_material,
        tool_wood_vm_material,
        tool_stone_vm_material,
        tool_iron_vm_material,
        tool_parchment_material,
        tool_parchment_vm_material,
        tool_cloth_material,
        tool_cloth_vm_material,
        tool_cord_material,
        tool_cord_vm_material,
        dropped_material: materials.add(StandardMaterial {
            base_color: DROPPED_BAG_COLOR,
            perceptual_roughness: 0.95,
            reflectance: 0.15,
            ..default()
        }),
        held_bag_material: materials.add(StandardMaterial {
            base_color: HELD_BAG_COLOR,
            perceptual_roughness: 0.88,
            reflectance: 0.15,
            ..default()
        }),
    };
    // Precompute the in-hand layer stacks for every held item from the declarative
    // `HeldMesh::visual` table, then hand both resources to the world. `held_item_layers`
    // (first-person and remote-player rig) is a plain map lookup into this.
    commands.insert_resource(crate::app::systems::build_held_item_visuals(
        asset_server,
        &item_visual_assets,
    ));
    commands.insert_resource(item_visual_assets);
}

/// Worn-armor rig visuals: the per-family PBR materials plus the `ArmorVisuals`
/// lookup the rig-attachment system reads.
fn insert_armor_visuals(
    commands: &mut Commands,
    asset_server: &AssetServer,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) {
    // Worn-armor rig visuals. Armor matches the PLAYER RIG's material family
    // (PBR `StandardMaterial`, not the cel/toon family the held tools use above),
    // so each family (cloth / wood slat / steel) is one shared `StandardMaterial`
    // built here from its detail texture; COLOR_0 on the glb carries per-piece
    // colour, the texture only adds surface grain, exactly how the rig itself
    // renders. `build_armor_visuals` then folds the declarative `ArmorMesh::visual`
    // table into the `ArmorVisuals` lookup the rig-attachment system reads. A new
    // armor set is one table row plus its glbs, no material or attachment code.
    let mut load_named_texture = |rel: &str| -> Handle<Image> {
        let bytes =
            embedded_bytes(rel).unwrap_or_else(|| panic!("embedded armor texture missing: {rel}"));
        let mut image = Image::from_buffer(
            bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::Descriptor(tree_texture_sampler()),
            RenderAssetUsages::RENDER_WORLD,
        )
        .unwrap_or_else(|err| panic!("decode armor texture {rel}: {err:?}"));
        build_mip_chain(&mut image);
        images.add(image)
    };
    let armor_materials = crate::app::systems::ArmorMaterials {
        // Matte woven cloth (padded set), roughness ~0.9 per the rendering-materials
        // doc; identity colour from the glb COLOR_0.
        cloth: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            base_color_texture: Some(load_named_texture("textures/tools/cloth.png")),
            perceptual_roughness: 0.92,
            reflectance: 0.12,
            ..default()
        }),
        // Matte hewn-wood slats (lamellar set), same matte roughness as the wood
        // building tiers.
        wood_slat: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            base_color_texture: Some(load_named_texture("textures/props/wood_slat.png")),
            perceptual_roughness: 0.9,
            reflectance: 0.13,
            ..default()
        }),
        // Forged steel plate (iron set): metallic with honest roughness so it
        // reads as plate catching the sky IBL, not a chrome mirror (matches the
        // iron-door material intent).
        steel: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            base_color_texture: Some(load_named_texture("textures/tools/steel.png")),
            metallic: 0.9,
            perceptual_roughness: 0.5,
            reflectance: 0.5,
            ..default()
        }),
    };
    commands.insert_resource(crate::app::systems::build_armor_visuals(
        asset_server,
        &armor_materials,
    ));
}

/// Resource-node visuals: the tree/ore textures and glbs, the LOD meshes, and
/// the shared cel materials, folded into `ResourceVisualAssets`.
fn insert_resource_node_visuals(
    commands: &mut Commands,
    asset_server: &AssetServer,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    toon_materials: &mut Assets<ToonMaterial>,
    images: &mut Assets<Image>,
    toon_no_glow_tex: &Handle<Image>,
) {
    // Tree textures: bark (tiles up the trunk) + canopy foliage detail. Both are
    // now soft, low-contrast PAINTED cel detail (the canopy is solid faceted
    // geometry, not alpha cards, so foliage is opaque too): the cel `ToonMaterial`
    // supplies the hard bands + ink edge, the texture only adds needle/leaf grain
    // that rides the glb COLOR_0, exactly like the ore rock detail. Decoded
    // synchronously with a CPU mip chain (Bevy 0.18 builds none for loaded PNGs;
    // without mips the grain aliases into sparkle at range) and a repeat +
    // anisotropic sampler so bark tiles vertically and the foliage tiles across the
    // cones/blobs. Loaded sRGB so the sampler hands the shader linear colour; the
    // glb COLOR_0 (linear) tints each layer. Mirrors `TerrainTextureAssets::load`.
    let mut load_tree_texture = |name: &str| -> Handle<Image> {
        let rel = format!("textures/trees/{name}.png");
        let bytes =
            embedded_bytes(&rel).unwrap_or_else(|| panic!("embedded tree texture missing: {rel}"));
        let mut image = Image::from_buffer(
            bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::Descriptor(tree_texture_sampler()),
            RenderAssetUsages::RENDER_WORLD,
        )
        .unwrap_or_else(|err| panic!("decode tree texture {rel}: {err:?}"));
        build_mip_chain(&mut image);
        images.add(image)
    };
    let pine_bark_tex = load_tree_texture("bark_pine");
    let pine_foliage_tex = load_tree_texture("foliage_pine");
    let birch_bark_tex = load_tree_texture("bark_birch");
    let birch_foliage_tex = load_tree_texture("foliage_birch");

    // Per-type ore albedo: the image-to-3D bake rebaked onto the low-poly
    // UVs (`art/ore/rework/build_nodes.py`), one texture per ore type shared
    // by that type's three depletion stages. The mineral identity (grey rock
    // vs rust/coal/sulfur chunks, meteorite slag) lives entirely in this
    // texture; the glb COLOR_0 is white. Decoded sRGB with a CPU mip chain +
    // aniso sampler, same as the tree textures.
    let mut load_ore_albedo = |ty: &str| -> Handle<Image> {
        let rel = format!("textures/ore/{ty}.png");
        let bytes =
            embedded_bytes(&rel).unwrap_or_else(|| panic!("embedded ore texture missing: {rel}"));
        let mut image = Image::from_buffer(
            bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::Descriptor(tree_texture_sampler()),
            RenderAssetUsages::RENDER_WORLD,
        )
        .unwrap_or_else(|err| panic!("decode ore texture {rel}: {err:?}"));
        build_mip_chain(&mut image);
        images.add(image)
    };
    let stone_albedo = load_ore_albedo("stone");
    let iron_albedo = load_ore_albedo("iron");
    let coal_albedo = load_ore_albedo("coal");
    let sulfur_albedo = load_ore_albedo("sulfur");
    let meteorite_albedo = load_ore_albedo("meteorite");

    // Each live tree is an authored Blender glb with two meshes: mesh 0 = the
    // bark trunk, mesh 1 = the canopy. Geometry + UVs + COLOR_0 come from the
    // model; the materials above carry the textures. Sources: `art/trees/*`.
    let pine_small_glb = embedded_asset_path("trees/pine_small/model.glb");
    let pine_medium_glb = embedded_asset_path("trees/pine_medium/model.glb");
    let pine_large_glb = embedded_asset_path("trees/pine_large/model.glb");
    let birch_small_glb = embedded_asset_path("trees/birch_small/model.glb");
    let birch_medium_glb = embedded_asset_path("trees/birch_medium/model.glb");
    let birch_large_glb = embedded_asset_path("trees/birch_large/model.glb");
    // Dead snags: a single bark-trunk + bare-branches mesh per size, no canopy.
    // Tinted weathered grey by `dead_bark_material` below.
    let dead_small_glb = embedded_asset_path("trees/dead_small/model.glb");
    let dead_medium_glb = embedded_asset_path("trees/dead_medium/model.glb");
    let dead_large_glb = embedded_asset_path("trees/dead_large/model.glb");
    let tree_prim = |glb: &str, mesh: usize| -> Handle<Mesh> {
        asset_server
            .load(GltfAssetLabel::Primitive { mesh, primitive: 0 }.from_asset(glb.to_owned()))
    };
    // Ore/vein nodes: one generated glb per (type, depletion stage), lean
    // meshes (UVs + white COLOR_0) whose per-mineral look lives in the baked
    // per-type albedo on the materials above. Stage index = mesh 0/1/2 glb.
    // See `art/ore/build_nodes.py`.
    let ore_stage_meshes = |ty: &str| -> [Handle<Mesh>; ORE_NODE_STAGE_COUNT] {
        std::array::from_fn(|stage| {
            asset_server.load(
                GltfAssetLabel::Primitive {
                    mesh: 0,
                    primitive: 0,
                }
                .from_asset(embedded_asset_path(&format!("ore/{ty}/stage_{stage}.glb"))),
            )
        })
    };
    let ore_material = |albedo: &Handle<Image>| ToonMaterial {
        detail: albedo.clone(),
        params: Vec4::new(3.0, 0.0, 0.8, 2.2),
        tex_scale: 1.0, // ore glbs carry their own UVs; triplanar scale unused
        fade: 1.0,
        dev_flags: 0,
        emissive_tex: toon_no_glow_tex.clone(),
        emissive: Vec4::ZERO,
    };
    commands.insert_resource(ResourceVisualAssets {
        coal_node_meshes: ore_stage_meshes("coal"),
        iron_node_meshes: ore_stage_meshes("iron"),
        sulfur_node_meshes: ore_stage_meshes("sulfur"),
        stone_vein_meshes: ore_stage_meshes("stone"),
        meteorite_node_meshes: ore_stage_meshes("meteorite"),
        pine_tree_small_trunk_mesh: tree_prim(&pine_small_glb, 0),
        pine_tree_small_foliage_mesh: tree_prim(&pine_small_glb, 1),
        pine_tree_medium_trunk_mesh: tree_prim(&pine_medium_glb, 0),
        pine_tree_medium_foliage_mesh: tree_prim(&pine_medium_glb, 1),
        pine_tree_large_trunk_mesh: tree_prim(&pine_large_glb, 0),
        pine_tree_large_foliage_mesh: tree_prim(&pine_large_glb, 1),
        birch_tree_small_trunk_mesh: tree_prim(&birch_small_glb, 0),
        birch_tree_small_foliage_mesh: tree_prim(&birch_small_glb, 1),
        birch_tree_medium_trunk_mesh: tree_prim(&birch_medium_glb, 0),
        birch_tree_medium_foliage_mesh: tree_prim(&birch_medium_glb, 1),
        birch_tree_large_trunk_mesh: tree_prim(&birch_large_glb, 0),
        birch_tree_large_foliage_mesh: tree_prim(&birch_large_glb, 1),
        pine_tree_small_lod_mesh: meshes.add(low_poly_pine_tree_small_lod_mesh()),
        pine_tree_medium_lod_mesh: meshes.add(low_poly_pine_tree_medium_lod_mesh()),
        pine_tree_large_lod_mesh: meshes.add(low_poly_pine_tree_large_lod_mesh()),
        birch_tree_small_lod_mesh: meshes.add(low_poly_birch_tree_small_lod_mesh()),
        birch_tree_medium_lod_mesh: meshes.add(low_poly_birch_tree_medium_lod_mesh()),
        birch_tree_large_lod_mesh: meshes.add(low_poly_birch_tree_large_lod_mesh()),
        dead_tree_small_mesh: tree_prim(&dead_small_glb, 0),
        dead_tree_medium_mesh: tree_prim(&dead_medium_glb, 0),
        dead_tree_large_mesh: tree_prim(&dead_large_glb, 0),
        surface_stone_mesh: meshes.add(low_poly_surface_stone_mesh()),
        branch_pile_mesh: meshes.add(low_poly_branch_pile_mesh()),
        // Bigger crossed tuft of the shared grass-card texture: clearly taller and
        // wider than a detail-grass card (~0.2-0.8 m) so the harvestable plant is easy
        // to spot for pickup in any biome.
        hay_grass_mesh: meshes.add(build_hay_tuft_mesh(1.0, 0.42, 3)),
        // Per-type cel-shaded ore materials over the baked albedos. params =
        // (cel band count, alpha cutoff, ink-edge strength, ink-edge width
        // exp): 3 bands + a strong dark silhouette edge, alpha cutoff 0
        // keeps the solid boulder opaque (only the grass-card tufts mask).
        stone_vein_material: toon_materials.add(ore_material(&stone_albedo)),
        iron_node_material: toon_materials.add(ore_material(&iron_albedo)),
        coal_node_material: toon_materials.add(ore_material(&coal_albedo)),
        sulfur_node_material: toon_materials.add(ore_material(&sulfur_albedo)),
        meteorite_node_material: toon_materials.add(ore_material(&meteorite_albedo)),
        vertex_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.98,
            reflectance: 0.12,
            ..default()
        }),
        // Three toony tall-grass tuft cards (taller, seed-headed, loosely splayed);
        // a hay node picks one by `id % 3` (see `resource_node_visual`) so the
        // harvestable plants vary instead of being one repeated card. Sources:
        // `art/textures/grass/tall_{1,2,3}.png` -> `textures/tall_grass_{1,2,3}.png`.
        hay_grass_materials: [
            toon_materials.add(hay_tall_grass_material(
                asset_server.load(embedded_asset_path("textures/tall_grass_1.png")),
                toon_no_glow_tex.clone(),
            )),
            toon_materials.add(hay_tall_grass_material(
                asset_server.load(embedded_asset_path("textures/tall_grass_2.png")),
                toon_no_glow_tex.clone(),
            )),
            toon_materials.add(hay_tall_grass_material(
                asset_server.load(embedded_asset_path("textures/tall_grass_3.png")),
                toon_no_glow_tex.clone(),
            )),
        ],
        // Trees join the cel family: trunk + canopy are now cel-shaded
        // `ToonMaterial`, matching the ore nodes + deployables. Each surface gets
        // its own painted detail texture (bark grain / needle / leaf) and the glb
        // COLOR_0 supplies the per-layer tone, exactly the `texture * COLOR_0`
        // trick the ore rock uses. Foliage is a SOLID faceted canopy (not alpha
        // cards), so it stays opaque and double-sided handling is moot. Trees are
        // rounded/organic like the ore, so they run the SOFTER ore-style cel (a
        // gentler ink edge than the boxy deployables). `fade` is 1.0; only the
        // felling dissolve clones one and drives it down. See `art/trees/*` +
        // docs/toon-shading.md.
        pine_bark_material: toon_materials.add(ToonMaterial {
            detail: pine_bark_tex.clone(),
            params: Vec4::new(3.0, 0.0, 0.55, 2.6),
            tex_scale: 1.0,
            fade: 1.0,
            dev_flags: 0,
            emissive_tex: toon_no_glow_tex.clone(),
            emissive: Vec4::ZERO,
        }),
        birch_bark_material: toon_materials.add(ToonMaterial {
            detail: birch_bark_tex,
            params: Vec4::new(3.0, 0.0, 0.55, 2.6),
            tex_scale: 1.0,
            fade: 1.0,
            dev_flags: 0,
            emissive_tex: toon_no_glow_tex.clone(),
            emissive: Vec4::ZERO,
        }),
        // Canopy: clean cel bands + a slightly wider ink edge so the leafy mass
        // reads with a drawn silhouette outline (the anime "sticker" look from the
        // references). The green rides the foliage detail texture; COLOR_0 layers
        // it dark (lower) -> light (crown).
        pine_foliage_material: toon_materials.add(ToonMaterial {
            detail: pine_foliage_tex,
            params: Vec4::new(3.0, 0.0, 0.7, 2.0),
            tex_scale: 1.0,
            fade: 1.0,
            dev_flags: 0,
            emissive_tex: toon_no_glow_tex.clone(),
            emissive: Vec4::ZERO,
        }),
        birch_foliage_material: toon_materials.add(ToonMaterial {
            detail: birch_foliage_tex,
            params: Vec4::new(3.0, 0.0, 0.7, 2.0),
            tex_scale: 1.0,
            fade: 1.0,
            dev_flags: 0,
            emissive_tex: toon_no_glow_tex.clone(),
            emissive: Vec4::ZERO,
        }),
        // Weathered dead bark: the same pine bark detail, but the dead-snag glb
        // carries a cool-grey COLOR_0 (set in build_tree.py) so `texture * COLOR_0`
        // reads grey and dead rather than like a live trunk. Cel-shaded like the
        // rest so a leafless snag still belongs to the family.
        dead_bark_material: toon_materials.add(ToonMaterial {
            detail: pine_bark_tex,
            params: Vec4::new(3.0, 0.0, 0.5, 2.6),
            tex_scale: 1.0,
            fade: 1.0,
            dev_flags: 0,
            emissive_tex: toon_no_glow_tex.clone(),
            emissive: Vec4::ZERO,
        }),
    });
}

/// Deployable + building visuals: the authored glbs, tier/door textures, the
/// placement-ghost materials, and the fuse-spark assets, folded into
/// `DeployableVisualAssets`.
fn insert_deployable_visuals(
    commands: &mut Commands,
    asset_server: &AssetServer,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    toon_materials: &mut Assets<ToonMaterial>,
    images: &mut Assets<Image>,
    toon_no_glow_tex: &Handle<Image>,
) {
    // One glb mesh primitive through the asset server: the DRY loader every
    // authored model below goes through.
    let prim_mesh = |glb: &str, primitive: usize| -> Handle<Mesh> {
        asset_server
            .load(GltfAssetLabel::Primitive { mesh: 0, primitive }.from_asset(glb.to_owned()))
    };
    // Hand-painted toon detail textures for the deployables, mapped by the
    // box-projected UVs baked into each model (`art/deployables/build_deployables.py`).
    // Near-white line-art grain (plank seams / cobble coursing / quilt weave) so
    // the prop's COLOR_0 sets the colour (wood brown / stone grey / fabric green),
    // the same base-white * COLOR_0 * texture trick as the bark + ore. Decoded
    // sRGB with a CPU mip chain + repeat/aniso sampler. Sources: `art/deployables/*`.
    let mut load_toon_texture = |name: &str| -> Handle<Image> {
        let rel = format!("textures/deployables/{name}.png");
        let bytes = embedded_bytes(&rel)
            .unwrap_or_else(|| panic!("embedded deployable texture missing: {rel}"));
        let mut image = Image::from_buffer(
            bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::Descriptor(tree_texture_sampler()),
            RenderAssetUsages::RENDER_WORLD,
        )
        .unwrap_or_else(|err| panic!("decode deployable texture {rel}: {err:?}"));
        build_mip_chain(&mut image);
        images.add(image)
    };
    let deployable_wood_tex = load_toon_texture("wood");
    let deployable_stone_tex = load_toon_texture("stone");
    let deployable_fabric_tex = load_toon_texture("fabric");

    // Placed structures are authored Blender glbs matching their inventory icons
    // (a splay-legged wooden bench, a cobblestone furnace with an arched glowing
    // mouth). Like the tools, each look is carried by the model's COLOR_0 vertex
    // colours; only the mesh primitive is loaded here, the shared `material` below
    // stays base-white so those vertex colours show through, exactly as the
    // procedural trees/ore nodes do. Sources:
    // `art/items/{workbench_t1,crude_furnace,storage_box_small,storage_box_large}/*.blend`.
    let workbench_glb = embedded_asset_path("items/workbench_t1/model.glb");
    let workbench_t2_glb = embedded_asset_path("items/workbench_t2/model.glb");
    let furnace_glb = embedded_asset_path("items/crude_furnace/model.glb");
    let storage_box_small_glb = embedded_asset_path("items/storage_box_small/model.glb");
    let storage_box_large_glb = embedded_asset_path("items/storage_box_large/model.glb");
    let torch_glb = embedded_asset_path("items/torch/model.glb");
    // Placed-charge glbs: the same 2-primitive models the held-item table loads
    // (body prim 0 + accent prim 1).
    let powder_keg_glb = embedded_asset_path("items/powder_keg/model.glb");
    let satchel_charge_glb = embedded_asset_path("items/satchel_charge/model.glb");
    let powder_bomb_glb = embedded_asset_path("items/powder_bomb/model.glb");
    let tool_cupboard_glb = embedded_asset_path("items/tool_cupboard/model.glb");
    let ruin_cache_glb = embedded_asset_path("ruins/ruin_cache_chest.glb");
    // Burnt-house shells, one glb per RuinPrefab in RuinPrefab::ALL order
    // (see `ruin_house_meshes` on the struct).
    let ruin_house_glbs = crate::world::RuinPrefab::ALL
        .map(|prefab| embedded_asset_path(&format!("ruins/{}.glb", prefab.asset_stem())));
    let sleeping_bag_glb = embedded_asset_path("items/sleeping_bag/model.glb");
    // Authored door panels (wood + iron, a matched family). Source:
    // `art/building/build_door.py`.
    let hewn_door_glb = embedded_asset_path("items/hewn_log_door/model.glb");
    let iron_door_glb = embedded_asset_path("items/iron_door/model.glb");
    // Building pieces are authored Blender glbs (UV-unwrapped + COLOR_0), one
    // per (piece, tier), matching the same box layout as the collider so the
    // visual silhouette agrees with what blocks movement. Source:
    // `art/building/build_pieces.py`. The tier texture is supplied by the
    // shared `building_materials` below. Filenames are `<piece>_<tier>.glb`.
    let piece_name = |piece| match piece {
        crate::building::BuildingPiece::Foundation => "foundation",
        crate::building::BuildingPiece::Wall => "wall",
        crate::building::BuildingPiece::WindowWall => "window_wall",
        crate::building::BuildingPiece::Doorway => "doorway",
        crate::building::BuildingPiece::Ceiling => "ceiling",
        crate::building::BuildingPiece::Stairs => "stairs",
    };
    let tier_name = |tier| match tier {
        crate::building::BuildingTier::Sticks => "sticks",
        crate::building::BuildingTier::HewnWood => "wood",
        crate::building::BuildingTier::Stone => "stone",
    };
    let building_meshes = [
        crate::building::BuildingPiece::Foundation,
        crate::building::BuildingPiece::Wall,
        crate::building::BuildingPiece::WindowWall,
        crate::building::BuildingPiece::Doorway,
        crate::building::BuildingPiece::Ceiling,
        crate::building::BuildingPiece::Stairs,
    ]
    .map(|piece| {
        [
            crate::building::BuildingTier::Sticks,
            crate::building::BuildingTier::HewnWood,
            crate::building::BuildingTier::Stone,
        ]
        .map(|tier| {
            let glb = embedded_asset_path(&format!(
                "building/{}_{}.glb",
                piece_name(piece),
                tier_name(tier)
            ));
            prim_mesh(&glb, 0)
        })
    });
    // Door surface textures (plank / forged plate), decoded with a CPU mip
    // chain + repeat/aniso sampler like the tree bark so the grain tiles and
    // stays crisp; the glb COLOR_0 multiplies on top to tint frame/braces.
    let mut load_building_texture = |name: &str| -> Handle<Image> {
        let rel = format!("textures/building/{name}.png");
        let bytes = embedded_bytes(&rel)
            .unwrap_or_else(|| panic!("embedded building texture missing: {rel}"));
        let mut image = Image::from_buffer(
            bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::Descriptor(tree_texture_sampler()),
            RenderAssetUsages::RENDER_WORLD,
        )
        .unwrap_or_else(|err| panic!("decode building texture {rel}: {err:?}"));
        build_mip_chain(&mut image);
        images.add(image)
    };
    let door_wood_tex = load_building_texture("door_wood");
    let door_iron_tex = load_building_texture("door_iron");
    // Building tier surface textures (twig lattice / hewn timber / coursed
    // stone), indexed by `BuildingTier` (sticks, hewn wood, stone).
    let sticks_tex = load_building_texture("sticks");
    let wood_tex = load_building_texture("wood");
    let stone_tex = load_building_texture("stone");
    // `uv_scale` < 1 enlarges the texture (fewer repeats across a 3 m piece),
    // tuned per tier so the stone coursing reads as big blocks (one repeat per
    // wall, not a busy tiled grid) while the wood/bark grain stays fine.
    let mut building_material =
        |tex: Handle<Image>, roughness: f32, reflectance: f32, uv_scale: f32| {
            materials.add(StandardMaterial {
                base_color: VERTEX_MATERIAL_COLOR,
                base_color_texture: Some(tex),
                perceptual_roughness: roughness,
                reflectance,
                uv_transform: bevy::math::Affine2::from_scale(Vec2::splat(uv_scale)),
                ..default()
            })
        };
    let building_materials = [
        building_material(sticks_tex, 0.95, 0.12, 1.0),
        building_material(wood_tex, 0.92, 0.13, 1.0),
        building_material(stone_tex, 0.95, 0.1, 0.5),
    ];
    // Shared bright-ember spark for every charge's fuse rig (an additive cube,
    // same recipe as the furnace spark). Built here so the fuse rig can spark.
    commands.insert_resource(crate::app::systems::ChargeFuseAssets {
        spark_mesh: meshes.add(Cuboid::new(0.03, 0.03, 0.03)),
        spark_material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.9, 0.55, 0.15),
            emissive: LinearRgba::rgb(9.0, 4.2, 0.8),
            alpha_mode: AlphaMode::Add,
            unlit: true,
            ..default()
        }),
    });
    commands.insert_resource(DeployableVisualAssets {
        workbench_mesh: prim_mesh(&workbench_glb, 0),
        workbench_t2_mesh: prim_mesh(&workbench_t2_glb, 0),
        furnace_mesh: prim_mesh(&furnace_glb, 0),
        building_meshes,
        building_materials,
        hewn_door_panel_mesh: prim_mesh(&hewn_door_glb, 0),
        iron_door_panel_mesh: prim_mesh(&iron_door_glb, 0),
        shutter_panel_mesh: meshes.add(crate::app::scene::mesh::shutter_panel_mesh()),
        // Wood door: matte plank surface. Iron door: rough forged metal, a
        // touch metallic so it picks up the sky IBL and reads as steel rather
        // than flat-dark; the dark plate texture drives the F0 tint.
        hewn_door_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            base_color_texture: Some(door_wood_tex),
            perceptual_roughness: 0.9,
            reflectance: 0.13,
            ..default()
        }),
        iron_door_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            base_color_texture: Some(door_iron_tex),
            perceptual_roughness: 0.55,
            metallic: 0.8,
            ..default()
        }),
        door_ghost_mesh: meshes.add(door_ghost_mesh()),
        shutter_ghost_mesh: meshes.add(crate::app::scene::mesh::shutter_ghost_mesh()),
        sleeping_bag_mesh: prim_mesh(&sleeping_bag_glb, 0),
        storage_box_small_mesh: prim_mesh(&storage_box_small_glb, 0),
        storage_box_large_mesh: prim_mesh(&storage_box_large_glb, 0),
        torch_mesh: prim_mesh(&torch_glb, 0),
        charge_keg_mesh: prim_mesh(&powder_keg_glb, 0),
        charge_satchel_mesh: prim_mesh(&satchel_charge_glb, 0),
        charge_bomb_mesh: prim_mesh(&powder_bomb_glb, 0),
        tool_cupboard_mesh: prim_mesh(&tool_cupboard_glb, 0),
        ruin_cache_mesh: prim_mesh(&ruin_cache_glb, 0),
        // Shell prim 0 = charred timber, prim 1 = stone plinth + rubble; the
        // build script authors the material slots in that order.
        ruin_house_meshes: [
            (
                prim_mesh(&ruin_house_glbs[0], 0),
                prim_mesh(&ruin_house_glbs[0], 1),
            ),
            (
                prim_mesh(&ruin_house_glbs[1], 0),
                prim_mesh(&ruin_house_glbs[1], 1),
            ),
            (
                prim_mesh(&ruin_house_glbs[2], 0),
                prim_mesh(&ruin_house_glbs[2], 1),
            ),
            (
                prim_mesh(&ruin_house_glbs[3], 0),
                prim_mesh(&ruin_house_glbs[3], 1),
            ),
        ],
        // Per-surface cel-shaded materials for the deployables: hand-painted
        // wood / stone / fabric line-art mapped by the models' baked box-projected
        // UVs (tex_scale is the dead triplanar fallback, unused now the props
        // carry UVs). The deployables are mostly flat-faced boxes, so they run a
        // PUNCHIER cel than the rounded ore nodes (more cel bands / a full-strength
        // + wider ink edge so every beveled corner reads as a drawn outline). See
        // docs/toon-shading.md "flat-surface cel banding". Ore keeps its softer
        // params. Alpha cutoff (params.y) stays 0: these are opaque solids, only
        // the grass-card tufts mask.
        toon_wood_material: toon_materials.add(ToonMaterial {
            detail: deployable_wood_tex.clone(),
            params: Vec4::new(3.0, 0.0, 1.0, 1.4),
            tex_scale: 1.5,
            fade: 1.0,
            dev_flags: 0,
            emissive_tex: toon_no_glow_tex.clone(),
            emissive: Vec4::ZERO,
        }),
        toon_stone_material: toon_materials.add(ToonMaterial {
            detail: deployable_stone_tex.clone(),
            params: Vec4::new(3.0, 0.0, 1.0, 1.4),
            tex_scale: 1.5,
            fade: 1.0,
            dev_flags: 0,
            emissive_tex: toon_no_glow_tex.clone(),
            emissive: Vec4::ZERO,
        }),
        toon_fabric_material: toon_materials.add(ToonMaterial {
            detail: deployable_fabric_tex.clone(),
            params: Vec4::new(3.0, 0.0, 1.0, 1.4),
            tex_scale: 1.5,
            fade: 1.0,
            dev_flags: 0,
            emissive_tex: toon_no_glow_tex.clone(),
            emissive: Vec4::ZERO,
        }),
        // Placed cloth charges (bomb + satchel) reuse the deployable fabric
        // line-art; each glb's COLOR_0 gives the wrap its colour.
        charge_cloth_material: toon_materials.add(ToonMaterial {
            detail: deployable_fabric_tex.clone(),
            params: Vec4::new(3.0, 0.0, 1.0, 1.4),
            tex_scale: 1.5,
            fade: 1.0,
            dev_flags: 0,
            emissive_tex: toon_no_glow_tex.clone(),
            emissive: Vec4::ZERO,
        }),
        ghost_valid_material: materials.add(StandardMaterial {
            // Translucent green: visible against grass + stone without
            // hiding the surface under the ghost. Alpha blending only,
            // no shadow casting, set on spawn so the preview never bakes
            // into the lighting pass.
            base_color: Color::srgba(0.36, 0.92, 0.42, 0.38),
            emissive: Color::srgb(0.06, 0.30, 0.10).into(),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.85,
            reflectance: 0.10,
            ..default()
        }),
        ghost_invalid_material: materials.add(StandardMaterial {
            base_color: Color::srgba(0.96, 0.32, 0.32, 0.42),
            emissive: Color::srgb(0.40, 0.06, 0.06).into(),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.85,
            reflectance: 0.10,
            ..default()
        }),
        // The charge-ghost variants: same hues, higher alpha + hotter
        // emissive, so the small keg/satchel preview reads through grass.
        ghost_valid_charge_material: materials.add(StandardMaterial {
            base_color: Color::srgba(0.36, 0.92, 0.42, 0.55),
            emissive: Color::srgb(0.14, 0.65, 0.22).into(),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.85,
            reflectance: 0.10,
            ..default()
        }),
        ghost_invalid_charge_material: materials.add(StandardMaterial {
            base_color: Color::srgba(0.96, 0.32, 0.32, 0.58),
            emissive: Color::srgb(0.70, 0.10, 0.10).into(),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.85,
            reflectance: 0.10,
            ..default()
        }),
    });
}

/// Gather/combat impact-burst particle assets (wood chips, stone shards, grass
/// blades, blood spray + pool).
fn insert_impact_effect_assets(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    commands.insert_resource(ImpactEffectAssets {
        wood_chip_mesh: meshes.add(impact_wood_chip_mesh()),
        stone_shard_mesh: meshes.add(impact_stone_shard_mesh()),
        // A tiny round droplet (the particle scale shrinks it to ~a few cm).
        blood_droplet_mesh: meshes.add(Sphere::new(0.06).mesh().ico(1).expect("valid ico")),
        // Unit disc, laid flat + scaled to a small pool at spawn.
        blood_splatter_mesh: meshes.add(Circle::new(1.0)),
        wood_chip_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.95,
            reflectance: 0.12,
            ..default()
        }),
        stone_shard_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.88,
            reflectance: 0.12,
            ..default()
        }),
        grass_blade_material: materials.add(StandardMaterial {
            // Multiplies through the mesh vertex colours, so we still
            // get the per-face lighting variety from the shard mesh but
            // tinted toward fresh-grass green.
            base_color: Color::srgb(0.42, 0.62, 0.22),
            perceptual_roughness: 0.92,
            reflectance: 0.12,
            ..default()
        }),
        blood_material: materials.add(StandardMaterial {
            // Rich, slightly wet-looking crimson for PvP hit spray. The red
            // multiplies through the shard mesh's vertex colours; a touch of
            // reflectance reads as a wet sheen, not glowing.
            base_color: Color::srgb(0.55, 0.03, 0.02),
            perceptual_roughness: 0.7,
            reflectance: 0.2,
            ..default()
        }),
    });
}

/// Explosion feedback VFX assets (debris shards, flash, fireball, smoke, and
/// the ground shockwave ring).
fn insert_explosion_effect_assets(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    // Explosion feedback VFX: debris shards (grey + ember mix), a bright flash,
    // and a smoke puff. Reuses the impact stone-shard silhouette for debris.
    commands.insert_resource(crate::app::systems::ExplosionEffectAssets {
        shard_mesh: meshes.add(impact_stone_shard_mesh()),
        shard_grey_material: materials.add(StandardMaterial {
            // Dark smoke-grey blasted debris.
            base_color: Color::srgb(0.20, 0.19, 0.18),
            perceptual_roughness: 0.95,
            reflectance: 0.08,
            ..default()
        }),
        shard_ember_material: materials.add(StandardMaterial {
            // Hot ember-orange fragments, faintly emissive so they read as
            // freshly-blasted embers among the grey.
            base_color: Color::srgb(0.75, 0.32, 0.08),
            emissive: LinearRgba::rgb(2.2, 0.7, 0.1),
            perceptual_roughness: 0.85,
            reflectance: 0.10,
            ..default()
        }),
        flash_mesh: meshes.add(Sphere::new(1.0).mesh().ico(2).expect("valid ico")),
        flash_material: materials.add(StandardMaterial {
            // A hard bright additive pop of light at ground zero.
            base_color: Color::srgb(1.0, 0.85, 0.55),
            emissive: LinearRgba::rgb(14.0, 8.0, 3.0),
            alpha_mode: AlphaMode::Add,
            unlit: true,
            ..default()
        }),
        smoke_mesh: meshes.add(Sphere::new(0.5).mesh().ico(1).expect("valid ico")),
        smoke_material: materials.add(StandardMaterial {
            // Translucent dark grey smoke; blends and fades as the puff grows.
            base_color: Color::srgba(0.14, 0.13, 0.12, 0.55),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 1.0,
            reflectance: 0.02,
            unlit: true,
            ..default()
        }),
        fireball_material: materials.add(StandardMaterial {
            // The roiling fire body: additive hot orange, dimmer than the
            // flash so the pop still reads inside it, HDR enough to bloom.
            base_color: Color::srgb(0.9, 0.45, 0.12),
            emissive: LinearRgba::rgb(6.5, 2.4, 0.4),
            alpha_mode: AlphaMode::Add,
            unlit: true,
            ..default()
        }),
        // A thin flat torus: the ground shockwave ring. Unit major radius so
        // the tick system's scale IS the ring radius in metres.
        ring_mesh: meshes.add(Torus::new(0.96, 1.0)),
        ring_material: materials.add(StandardMaterial {
            // A pale dusty pressure ring, additive and faint so it reads as a
            // racing disturbance rather than a solid hoop.
            base_color: Color::srgb(0.9, 0.82, 0.66),
            emissive: LinearRgba::rgb(1.6, 1.4, 1.0),
            alpha_mode: AlphaMode::Add,
            unlit: true,
            ..default()
        }),
    });
}

/// Fire particle assets: the furnace flame/spark set, the torch flame + its
/// distance billboard, and the meteor's dedicated ember/smoke set.
fn insert_fire_particle_assets(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    commands.insert_resource(FurnaceFireAssets {
        // A small round puff. The fire is built from a dense stream of these
        // rising and fading; additive blending fuses the overlap into a soft
        // glowing flame body, so no single particle reads as a hard shape.
        // One ico subdivision (80 tris) instead of Bevy's 720-tri default:
        // ~50 puffs are alive per lit furnace, all in the transparent phase,
        // and a featureless additive glow blob can't show the difference.
        flame_mesh: meshes.add(Sphere::new(0.07).mesh().ico(1).expect("valid subdivisions")),
        flame_material: materials.add(StandardMaterial {
            // Additive + unlit so each puff paints pure glow over whatever's
            // behind it; the HDR (>1) emissive drives the bloom that gives the
            // accumulated core its heat under the filmic tonemap. Kept modest
            // per-puff because dozens overlap at the base.
            base_color: Color::srgb(0.55, 0.18, 0.03),
            emissive: LinearRgba::rgb(4.0, 1.4, 0.25),
            alpha_mode: AlphaMode::Add,
            unlit: true,
            ..default()
        }),
        spark_mesh: meshes.add(Cuboid::new(0.035, 0.035, 0.035)),
        spark_material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.8, 0.45, 0.12),
            emissive: LinearRgba::rgb(9.0, 4.2, 0.8),
            alpha_mode: AlphaMode::Add,
            unlit: true,
            ..default()
        }),
    });
    commands.insert_resource(TorchFireAssets {
        // A small additive puff, lighter than the furnace's (the torch flame
        // is a thin tongue, not a forge bed): one ico subdivision, only a
        // handful alive per torch at any time.
        flame_mesh: meshes.add(Sphere::new(0.05).mesh().ico(1).expect("valid subdivisions")),
        flame_material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.55, 0.20, 0.04),
            emissive: LinearRgba::rgb(3.4, 1.2, 0.22),
            alpha_mode: AlphaMode::Add,
            unlit: true,
            ..default()
        }),
        // The distance LOD: a single upright quad (in its local XY plane,
        // facing +Z) that the torch-fire system yaws toward the camera. Read
        // as the "bright rectangle" that stands in for the flame far away.
        billboard_mesh: meshes.add(Rectangle::new(0.16, 0.30)),
        billboard_material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.6, 0.24, 0.05),
            emissive: LinearRgba::rgb(4.6, 1.7, 0.30),
            alpha_mode: AlphaMode::Add,
            unlit: true,
            // The quad is double-sided so the billboard reads from either
            // face without the yaw needing to pick a winding.
            cull_mode: None,
            ..default()
        }),
    });
    commands.insert_resource(MeteorEmberAssets {
        // A spark point: the meteor sheds a dense stream of these off the tail, so
        // keep the mesh cheap (ico(1), 80 tris) and let the additive HDR base colour
        // carry the read. Bumped from 0.05 to 0.12 so each shed ember is a visible
        // glowing chunk, not a sub-pixel dust mote.
        ember_mesh: meshes.add(Sphere::new(0.12).mesh().ico(1).expect("valid subdivisions")),
        ember_material: materials.add(StandardMaterial {
            // Additive + unlit HDR ember: bright SATURATED orange. Under AgX a big
            // bright warm area washes to cream, but these sparks are TINY, so a
            // moderately bright orange (high red, ~0.3 green, ~0 blue) blooms into
            // clearly-orange glowing specks streaming off the tail rather than white
            // dots. Kept saturated (low green, zero blue) so they stay ember, not
            // pale-yellow dust.
            base_color: Color::linear_rgb(6.0, 1.05, 0.02),
            emissive: LinearRgba::rgb(7.2, 1.25, 0.02),
            alpha_mode: AlphaMode::Add,
            unlit: true,
            fog_enabled: false,
            ..default()
        }),
        // A soft round puff for the faint smoke shed under the fire trail.
        smoke_mesh: meshes.add(Sphere::new(0.5).mesh().ico(1).expect("valid subdivisions")),
        smoke_material: materials.add(StandardMaterial {
            // Translucent warm-dark grey; blends and fades as the puff grows,
            // reading as a thin smoke ribbon under the fire rather than a solid
            // shape. Fog off so a distant plume does not haze out.
            base_color: Color::srgba(0.09, 0.07, 0.06, 0.45),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 1.0,
            reflectance: 0.02,
            fog_enabled: false,
            cull_mode: None,
            unlit: true,
            ..default()
        }),
        // Unit radius so the airburst spawner scales it straight to world
        // metres. ico(3) keeps the silhouette round even at the flash's
        // multi-ball-radius peak.
        flash_mesh: meshes.add(Sphere::new(1.0).mesh().ico(3).expect("valid subdivisions")),
    });
}

pub(crate) fn player_visual_position(feet_position: Vec3) -> Vec3 {
    feet_position + Vec3::Y * PLAYER_VISUAL_CENTER_Y
}

pub(crate) fn menu_backdrop_depth_of_field() -> DepthOfField {
    DepthOfField {
        mode: DepthOfFieldMode::Gaussian,
        focal_distance: 0.35,
        aperture_f_stops: 0.08,
        max_depth: 80.0,
        ..default()
    }
}
