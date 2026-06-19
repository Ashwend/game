use bevy::{
    asset::RenderAssetUsages,
    audio::SpatialListener,
    core_pipeline::tonemapping::Tonemapping,
    gltf::GltfAssetLabel,
    image::{
        CompressedImageFormats, ImageAddressMode, ImageFilterMode, ImageSampler,
        ImageSamplerDescriptor, ImageType,
    },
    light::AtmosphereEnvironmentMapLight,
    pbr::{Atmosphere, AtmosphereSettings, ScatteringMedium},
    post_process::dof::{DepthOfField, DepthOfFieldMode},
    prelude::*,
    render::view::{Hdr, NoIndirectDrawing},
};

use super::mesh::builder::build_hay_tuft_mesh;
use super::terrain::build_mip_chain;
use super::{
    components::MainCamera,
    mesh::{
        COAL_ORE, IRON_ORE, ORE_NODE_STAGE_COUNT, PlayerRigMeshes, STONE_VEIN, SULFUR_ORE,
        build_player_rig_meshes, building_piece_mesh, door_ghost_mesh, door_panel_mesh,
        held_building_plan_mesh, held_hammer_mesh, impact_stone_shard_mesh, impact_wood_chip_mesh,
        low_poly_bag_mesh, low_poly_birch_tree_large_lod_mesh, low_poly_birch_tree_medium_lod_mesh,
        low_poly_birch_tree_small_lod_mesh, low_poly_branch_pile_mesh,
        low_poly_ore_node_stage_meshes, low_poly_pine_tree_large_lod_mesh,
        low_poly_pine_tree_medium_lod_mesh, low_poly_pine_tree_small_lod_mesh,
        low_poly_surface_stone_mesh, sleeping_bag_mesh,
    },
    sky::{initial_distance_fog, setup_sky},
};
use crate::app::embedded_assets::embedded_bytes;

use crate::app::{EYE_HEIGHT, PLAYER_VISUAL_CENTER_Y, embedded_asset_path};

/// Strength of the image-based ambient/reflection light generated from the
/// procedural sky. The sun is kept at a daylight-calibrated illuminance (see
/// `SUN_PEAK_ILLUMINANCE` in `sky.rs`) with the renderer's default exposure, so
/// the physical default of `1.0` reads well here and gives the scene consistent
/// ambient across the whole day. Lower it for moodier, deeper shadows.
pub(crate) const ATMOSPHERE_AMBIENT_INTENSITY: f32 = 1.0;

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
    pub(crate) held_bag_mesh: Handle<Mesh>,
    /// Both tool tiers (stone and iron) are authored Blender glbs matching their
    /// inventory icons. Each renders as two overlaid layers sharing one swing
    /// transform: a matte `*_body` (the wood haft plus its leather/twine
    /// bindings) and a `*_head` (worked stone or forged iron, the latter shiny).
    /// Both the geometry *and* the materials come from the glb's two primitives
    /// (see `setup_scene`), so the whole look is owned by the model.
    pub(crate) held_stone_hatchet_body_mesh: Handle<Mesh>,
    pub(crate) held_stone_hatchet_head_mesh: Handle<Mesh>,
    pub(crate) held_stone_pickaxe_body_mesh: Handle<Mesh>,
    pub(crate) held_stone_pickaxe_head_mesh: Handle<Mesh>,
    pub(crate) held_iron_hatchet_body_mesh: Handle<Mesh>,
    pub(crate) held_iron_hatchet_head_mesh: Handle<Mesh>,
    pub(crate) held_iron_pickaxe_body_mesh: Handle<Mesh>,
    pub(crate) held_iron_pickaxe_head_mesh: Handle<Mesh>,
    /// Per-tool materials carried by each glb (matte haft + stone/iron head),
    /// tinted by the model's COLOR_0 vertex colours. Sources:
    /// `art/items/{wood_stone,iron}_{hatchet,pickaxe}/*.blend`.
    pub(crate) held_stone_hatchet_body_material: Handle<StandardMaterial>,
    pub(crate) held_stone_hatchet_head_material: Handle<StandardMaterial>,
    pub(crate) held_stone_pickaxe_body_material: Handle<StandardMaterial>,
    pub(crate) held_stone_pickaxe_head_material: Handle<StandardMaterial>,
    pub(crate) held_iron_hatchet_body_material: Handle<StandardMaterial>,
    pub(crate) held_iron_hatchet_head_material: Handle<StandardMaterial>,
    pub(crate) held_iron_pickaxe_body_material: Handle<StandardMaterial>,
    pub(crate) held_iron_pickaxe_head_material: Handle<StandardMaterial>,
    pub(crate) dropped_material: Handle<StandardMaterial>,
    pub(crate) held_bag_material: Handle<StandardMaterial>,
    /// Procedural construction-hammer and building-plan viewmodels.
    /// Vertex-coloured like the world props; candidates for the authored
    /// glb pipeline later.
    pub(crate) held_hammer_mesh: Handle<Mesh>,
    pub(crate) held_building_plan_mesh: Handle<Mesh>,
    pub(crate) held_vertex_material: Handle<StandardMaterial>,
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
    pub(crate) coal_material: Handle<StandardMaterial>,
    pub(crate) iron_material: Handle<StandardMaterial>,
    pub(crate) sulfur_material: Handle<StandardMaterial>,
    pub(crate) stone_vein_material: Handle<StandardMaterial>,
    pub(crate) vertex_material: Handle<StandardMaterial>,
    /// Warm, alpha-masked material for the harvestable hay tuft: the shared
    /// grass-tuft texture tinted toward straw so it reads distinct from the
    /// cosmetic detail grass (see [`build_hay_tuft_mesh`]).
    pub(crate) hay_grass_material: Handle<StandardMaterial>,
    /// Tree bark (opaque, repeat-tiled, mipped) and canopy foliage (alpha-mask
    /// needle/leaf, double-sided, mipped) materials shared by every instance of
    /// a species so the forest batches by mesh+material. Built from the embedded
    /// `textures/trees/*.png` (see `load_tree_texture`). The glb's COLOR_0
    /// vertex colours tint these base-white materials per canopy layer.
    pub(crate) pine_bark_material: Handle<StandardMaterial>,
    pub(crate) pine_foliage_material: Handle<StandardMaterial>,
    pub(crate) birch_bark_material: Handle<StandardMaterial>,
    pub(crate) birch_foliage_material: Handle<StandardMaterial>,
    /// Weathered grey-brown bark for the dead-snag trunks: the pine bark texture
    /// multiplied by a desaturated cool-grey tint so a leafless tree reads as
    /// "dead", not just a live trunk without leaves.
    pub(crate) dead_bark_material: Handle<StandardMaterial>,
}

#[derive(Resource, Clone)]
pub(crate) struct DeployableVisualAssets {
    pub(crate) workbench_mesh: Handle<Mesh>,
    pub(crate) furnace_mesh: Handle<Mesh>,
    /// Building piece meshes indexed `[piece][tier]` via
    /// [`Self::building_mesh`]. Procedural composites built from the same
    /// boxes as the collision grid, so visuals and collision agree.
    pub(crate) building_meshes: [[Handle<Mesh>; 3]; 6],
    /// Door panel (hinge at origin, spans +X), spawned as an animated
    /// child of the door root entity.
    pub(crate) door_panel_mesh: Handle<Mesh>,
    /// Door placement ghost: closed panel + swing-arc indicator.
    pub(crate) door_ghost_mesh: Handle<Mesh>,
    pub(crate) sleeping_bag_mesh: Handle<Mesh>,
    /// Authored storage box models (Blender glbs, vertex-coloured like
    /// the workbench/furnace).
    pub(crate) storage_box_small_mesh: Handle<Mesh>,
    pub(crate) storage_box_large_mesh: Handle<Mesh>,
    /// Procedural torch haft + head (origin at the base so it mounts on the
    /// ground or tilts off a wall about its foot).
    pub(crate) torch_mesh: Handle<Mesh>,
    /// Authored Tool Cupboard model (Blender glb, vertex-coloured like the
    /// workbench/furnace; origin at the base so it sits on a foundation).
    pub(crate) tool_cupboard_mesh: Handle<Mesh>,
    /// Shared material used for placed structures. Vertex colours from
    /// the mesh do the heavy lifting; the material just supplies PBR
    /// reflectance + roughness so the wood/stone reads correctly under
    /// the day/night sun.
    pub(crate) material: Handle<StandardMaterial>,
    /// Semi-transparent green tint used by the placement ghost when the
    /// slot is valid. Mirrors the convention from popular survival games
    ///, green means "click to place", we pair it with a slight pulse.
    pub(crate) ghost_valid_material: Handle<StandardMaterial>,
    /// Red variant for invalid placement (out of reach, overlapping).
    pub(crate) ghost_invalid_material: Handle<StandardMaterial>,
}

impl DeployableVisualAssets {
    pub(crate) fn building_mesh(
        &self,
        piece: crate::building::BuildingPiece,
        tier: crate::building::BuildingTier,
    ) -> Handle<Mesh> {
        use crate::building::{BuildingPiece, BuildingTier};
        let piece_index = match piece {
            BuildingPiece::Foundation => 0,
            BuildingPiece::Wall => 1,
            BuildingPiece::WindowWall => 2,
            BuildingPiece::Doorway => 3,
            BuildingPiece::Ceiling => 4,
            BuildingPiece::Stairs => 5,
        };
        let tier_index = match tier {
            BuildingTier::Sticks => 0,
            BuildingTier::HewnWood => 1,
            BuildingTier::Stone => 2,
        };
        self.building_meshes[piece_index][tier_index].clone()
    }

    /// Storage box mesh for a tier (1 = small, 2+ = large).
    pub(crate) fn storage_box_mesh(&self, tier: u8) -> Handle<Mesh> {
        if tier >= 2 {
            self.storage_box_large_mesh.clone()
        } else {
            self.storage_box_small_mesh.clone()
        }
    }
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

/// Repeat + anisotropic trilinear sampler for the tree bark/canopy textures, so
/// bark tiles up the trunk and the needle/leaf texture tiles across the canopy
/// shells without a visible seam, and stays crisp (not shimmery) at distance.
/// Mirrors the terrain ground sampler; only meaningful with a mip chain
/// (`build_mip_chain`), which the tree-texture loader builds.
fn tree_texture_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    }
}

pub(crate) fn setup_scene(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut scattering_media: ResMut<Assets<ScatteringMedium>>,
) {
    // The four shared per-biome ground textures for the textured terrain floor.
    // Decoded + mip-chained once here; each world bakes only its own small
    // biome-weight raster when its ground spawns (see `super::world::spawn_world_geometry`).
    commands.insert_resource(super::TerrainTextureAssets::load(&mut images));
    // Ambient and clear color are now driven by the day/night cycle in
    // `sky::update_sky_system`. We still insert defaults here so the
    // very first frame (before the system runs) has sensible values
    // rather than the engine defaults.
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.72, 0.78, 0.86),
        brightness: 90.0,
        ..default()
    });

    commands.spawn((
        Name::new("Camera"),
        MainCamera,
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
        // Procedural physically-based sky. `earthlike` uses the default
        // earthlike scattering medium; `AtmosphereSettings` is auto-required
        // with sensible defaults (scene units are already metres). The
        // atmosphere reads the sun `DirectionalLight` to place the sun disc and
        // tint sunlight through the air, and renders the sky behind all
        // geometry, so the old hand-authored `ClearColor` sky is retired.
        Atmosphere::earthlike(scattering_media.add(ScatteringMedium::default())),
        // The atmosphere recomputes its LUTs every frame (no skip-if-unchanged
        // gating in Bevy 0.18), so these are a per-frame GPU cost. We trim them
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
    ));

    setup_sky(&mut commands, &mut meshes, &mut materials);

    commands.insert_resource(super::world::WorldSceneState::default());
    commands.insert_resource(PlayerVisualAssets {
        rig: build_player_rig_meshes(&mut meshes),
        remote_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.92,
            reflectance: 0.2,
            ..default()
        }),
    });
    // Every tool ships as an authored Blender glb (matching its inventory icon).
    // Both geometry *and* materials load straight from each model: two primitives
    // -> the two layers every tool uses (primitive 0 = matte wooden haft, with the
    // leather/twine bindings; primitive 1 = the worked stone or forged iron head),
    // and two base-white materials tinted by the model's COLOR_0 vertex colours. So
    // the whole look is owned by the asset. Sources:
    // `art/items/{wood_stone,iron}_{pickaxe,hatchet}/*.blend`.
    let stone_pickaxe_glb = embedded_asset_path("items/wood_stone_pickaxe/model.glb");
    let stone_hatchet_glb = embedded_asset_path("items/wood_stone_hatchet/model.glb");
    let pickaxe_glb = embedded_asset_path("items/iron_pickaxe/model.glb");
    let hatchet_glb = embedded_asset_path("items/iron_hatchet/model.glb");
    let prim_mesh = |glb: &str, primitive: usize| -> Handle<Mesh> {
        asset_server
            .load(GltfAssetLabel::Primitive { mesh: 0, primitive }.from_asset(glb.to_owned()))
    };
    let glb_material = |glb: &str, index: usize| -> Handle<StandardMaterial> {
        asset_server.load(
            GltfAssetLabel::Material {
                index,
                is_scale_inverted: false,
            }
            .from_asset(glb.to_owned()),
        )
    };
    commands.insert_resource(ItemVisualAssets {
        dropped_mesh: meshes.add(low_poly_bag_mesh()),
        held_bag_mesh: meshes.add(Cuboid::new(0.26, 0.22, 0.34)),
        held_stone_hatchet_body_mesh: prim_mesh(&stone_hatchet_glb, 0),
        held_stone_hatchet_head_mesh: prim_mesh(&stone_hatchet_glb, 1),
        held_stone_pickaxe_body_mesh: prim_mesh(&stone_pickaxe_glb, 0),
        held_stone_pickaxe_head_mesh: prim_mesh(&stone_pickaxe_glb, 1),
        held_iron_hatchet_body_mesh: prim_mesh(&hatchet_glb, 0),
        held_iron_hatchet_head_mesh: prim_mesh(&hatchet_glb, 1),
        held_iron_pickaxe_body_mesh: prim_mesh(&pickaxe_glb, 0),
        held_iron_pickaxe_head_mesh: prim_mesh(&pickaxe_glb, 1),
        held_stone_hatchet_body_material: glb_material(&stone_hatchet_glb, 0),
        held_stone_hatchet_head_material: glb_material(&stone_hatchet_glb, 1),
        held_stone_pickaxe_body_material: glb_material(&stone_pickaxe_glb, 0),
        held_stone_pickaxe_head_material: glb_material(&stone_pickaxe_glb, 1),
        held_iron_hatchet_body_material: glb_material(&hatchet_glb, 0),
        held_iron_hatchet_head_material: glb_material(&hatchet_glb, 1),
        held_iron_pickaxe_body_material: glb_material(&pickaxe_glb, 0),
        held_iron_pickaxe_head_material: glb_material(&pickaxe_glb, 1),
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
        held_hammer_mesh: meshes.add(held_hammer_mesh()),
        held_building_plan_mesh: meshes.add(held_building_plan_mesh()),
        held_vertex_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.90,
            reflectance: 0.15,
            ..default()
        }),
    });
    // Tree textures: bark (opaque, tiles up the trunk) + canopy needle/leaf
    // (alpha-mask). Decoded synchronously with a CPU mip chain (Bevy 0.18 builds
    // none for loaded PNGs; without mips the masked canopy aliases into sparkle at
    // range) and a repeat + anisotropic sampler so bark tiles vertically and the
    // needles/leaves tile across the cones/blobs. Loaded sRGB so the sampler hands
    // the shader linear colour; the glb COLOR_0 vertex colours (linear) tint each
    // canopy layer / the trunk on top. Mirrors `TerrainTextureAssets::load`.
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
    let pine_needle_tex = load_tree_texture("needles");
    let birch_bark_tex = load_tree_texture("bark_birch");
    let birch_leaf_tex = load_tree_texture("leaves");

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
    commands.insert_resource(ResourceVisualAssets {
        coal_node_meshes: low_poly_ore_node_stage_meshes(COAL_ORE).map(|mesh| meshes.add(mesh)),
        iron_node_meshes: low_poly_ore_node_stage_meshes(IRON_ORE).map(|mesh| meshes.add(mesh)),
        sulfur_node_meshes: low_poly_ore_node_stage_meshes(SULFUR_ORE).map(|mesh| meshes.add(mesh)),
        stone_vein_meshes: low_poly_ore_node_stage_meshes(STONE_VEIN).map(|mesh| meshes.add(mesh)),
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
        coal_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.98,
            reflectance: 0.12,
            ..default()
        }),
        iron_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.78,
            metallic: 0.18,
            ..default()
        }),
        sulfur_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.88,
            reflectance: 0.12,
            ..default()
        }),
        stone_vein_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.95,
            reflectance: 0.12,
            ..default()
        }),
        vertex_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.98,
            reflectance: 0.12,
            ..default()
        }),
        hay_grass_material: materials.add(StandardMaterial {
            // Same grass-tuft texture as the detail grass, alpha-masked for the blade
            // silhouette, but a richer/deeper green than the field plus a faint pop so
            // the harvestable tuft is locatable. (Hay spawns on the hay channel =
            // plains, where the detail grass is biome-tinted yellow, so a green tuft
            // reads clearly there; the bigger size, see `build_hay_tuft_mesh` above,
            // distinguishes it in greener biomes. A fixed colour can't beat the field
            // in *every* biome since the field's colour varies, so size carries the
            // rest.) Thin ribbons: both faces, matte, near-zero reflectance.
            // A lush, saturated deep green (deliberately NOT yellow-green: a high
            // red channel reads washed/sickly against the field). Green dominates
            // with red and blue held well below it for saturation; the overall
            // luminance matches the previous tint so the tuft stays just as easy
            // to spot. This is the tip brightness, the mesh's root→tip vertex
            // gradient (see `build_hay_tuft_mesh`) multiplies the base darker.
            base_color: Color::srgb(0.36, 0.70, 0.30),
            base_color_texture: Some(
                asset_server.load(embedded_asset_path("textures/grass_tuft.png")),
            ),
            // Faint green self-lift (not yellow) so the tuft reads with a little
            // life under dim dusk/night light without glowing.
            emissive: LinearRgba::rgb(0.008, 0.020, 0.006),
            alpha_mode: AlphaMode::Mask(0.4),
            cull_mode: None,
            perceptual_roughness: 0.95,
            reflectance: 0.04,
            ..default()
        }),
        // Bark: opaque, repeat-tiled mipped texture, matte dry-organic PBR
        // (materials.md). Base white so the trunk's COLOR_0 (near-white, with a
        // touch of ground-contact AO on the base ring) reads the bark texture.
        pine_bark_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            base_color_texture: Some(pine_bark_tex.clone()),
            perceptual_roughness: 0.95,
            reflectance: 0.12,
            ..default()
        }),
        birch_bark_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            base_color_texture: Some(birch_bark_tex),
            perceptual_roughness: 0.95,
            reflectance: 0.12,
            ..default()
        }),
        // Canopy: alpha-masked (never blended, so it stays in the cheap opaque
        // pass at forest scale), double-sided like the hay tuft so needles/leaves
        // read from both faces, matte. Mask cutoff combines the texture alpha with
        // the canopy's COLOR_0 alpha (low at the bottom rim) to feather the cone
        // skirt instead of leaving a hard disc edge. Per-layer COLOR_0 rgb tints
        // dark (lower) -> light (crown).
        pine_foliage_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            base_color_texture: Some(pine_needle_tex),
            alpha_mode: AlphaMode::Mask(0.4),
            cull_mode: None,
            perceptual_roughness: 0.95,
            reflectance: 0.12,
            ..default()
        }),
        birch_foliage_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            base_color_texture: Some(birch_leaf_tex),
            alpha_mode: AlphaMode::Mask(0.4),
            cull_mode: None,
            perceptual_roughness: 0.95,
            reflectance: 0.12,
            ..default()
        }),
        // Weathered dead bark: the pine bark texture tinted by a desaturated
        // cool-grey base colour (multiplied through) so a leafless snag reads grey
        // and dead rather than like a live trunk. A bit rougher than live bark.
        dead_bark_material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.50, 0.49, 0.47),
            base_color_texture: Some(pine_bark_tex),
            perceptual_roughness: 0.97,
            reflectance: 0.10,
            ..default()
        }),
    });
    // Placed structures are authored Blender glbs matching their inventory icons
    // (a splay-legged wooden bench, a cobblestone furnace with an arched glowing
    // mouth). Like the tools, each look is carried by the model's COLOR_0 vertex
    // colours; only the mesh primitive is loaded here, the shared `material` below
    // stays base-white so those vertex colours show through, exactly as the
    // procedural trees/ore nodes do. Sources:
    // `art/items/{workbench_t1,crude_furnace,storage_box_small,storage_box_large}/*.blend`.
    let workbench_glb = embedded_asset_path("items/workbench_t1/model.glb");
    let furnace_glb = embedded_asset_path("items/crude_furnace/model.glb");
    let storage_box_small_glb = embedded_asset_path("items/storage_box_small/model.glb");
    let storage_box_large_glb = embedded_asset_path("items/storage_box_large/model.glb");
    let torch_glb = embedded_asset_path("items/torch/model.glb");
    let tool_cupboard_glb = embedded_asset_path("items/tool_cupboard/model.glb");
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
        .map(|tier| meshes.add(building_piece_mesh(piece, tier)))
    });
    commands.insert_resource(DeployableVisualAssets {
        workbench_mesh: prim_mesh(&workbench_glb, 0),
        furnace_mesh: prim_mesh(&furnace_glb, 0),
        building_meshes,
        door_panel_mesh: meshes.add(door_panel_mesh()),
        door_ghost_mesh: meshes.add(door_ghost_mesh()),
        sleeping_bag_mesh: meshes.add(sleeping_bag_mesh()),
        storage_box_small_mesh: prim_mesh(&storage_box_small_glb, 0),
        storage_box_large_mesh: prim_mesh(&storage_box_large_glb, 0),
        torch_mesh: prim_mesh(&torch_glb, 0),
        tool_cupboard_mesh: prim_mesh(&tool_cupboard_glb, 0),
        material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.92,
            reflectance: 0.15,
            ..default()
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
    });
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
