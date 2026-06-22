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
use super::toon::ToonMaterial;
use super::{
    components::MainCamera,
    mesh::{
        ORE_NODE_STAGE_COUNT, PlayerRigMeshes, build_player_rig_meshes, door_ghost_mesh,
        held_building_plan_mesh, held_hammer_mesh, impact_stone_shard_mesh, impact_wood_chip_mesh,
        low_poly_bag_mesh, low_poly_birch_tree_large_lod_mesh, low_poly_birch_tree_medium_lod_mesh,
        low_poly_birch_tree_small_lod_mesh, low_poly_branch_pile_mesh,
        low_poly_pine_tree_large_lod_mesh, low_poly_pine_tree_medium_lod_mesh,
        low_poly_pine_tree_small_lod_mesh, low_poly_surface_stone_mesh,
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

/// Build one toony tall-grass (hay) material from a seed-headed tuft card. The
/// near-white base lets the painted texture's bright green read; alpha-masked
/// for the blade silhouette, double-sided thin ribbons, matte, with a faint
/// green self-lift so the harvestable plant stays visible at dusk.
fn hay_tall_grass_material(tex: Handle<Image>) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgb(0.86, 0.97, 0.80),
        base_color_texture: Some(tex),
        emissive: LinearRgba::rgb(0.008, 0.020, 0.006),
        alpha_mode: AlphaMode::Mask(0.4),
        cull_mode: None,
        perceptual_roughness: 0.95,
        reflectance: 0.04,
        ..default()
    }
}

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
    /// One shared cel-shaded material for all four ore/vein nodes: the per-mineral
    /// colour rides on the glb COLOR_0, so the rock texture + toon ramp are shared
    /// (see [`ToonMaterial`] and `art/ore/build_ore.py`).
    pub(crate) ore_toon_material: Handle<ToonMaterial>,
    pub(crate) vertex_material: Handle<StandardMaterial>,
    /// Warm, alpha-masked material for the harvestable hay tuft: the shared
    /// grass-tuft texture tinted toward straw so it reads distinct from the
    /// cosmetic detail grass (see [`build_hay_tuft_mesh`]).
    /// Three toony tall-grass materials (each a different seed-headed tuft card);
    /// a hay node picks one by `id % 3` so the harvestable plants vary.
    pub(crate) hay_grass_materials: [Handle<StandardMaterial>; 3],
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

#[derive(Resource, Clone)]
pub(crate) struct DeployableVisualAssets {
    pub(crate) workbench_mesh: Handle<Mesh>,
    pub(crate) furnace_mesh: Handle<Mesh>,
    /// Building piece meshes indexed `[piece][tier]` via
    /// [`Self::building_mesh`]. Authored Blender glbs built from the same box
    /// layout as the collision grid, so the silhouette agrees with what
    /// blocks movement. Source: `art/building/build_pieces.py`.
    pub(crate) building_meshes: [[Handle<Mesh>; 3]; 6],
    /// Textured tier materials (twig / hewn timber / coursed stone) indexed by
    /// [`crate::building::BuildingTier`], applied to every building piece of
    /// that tier (the glb COLOR_0 multiplies them).
    pub(crate) building_materials: [Handle<StandardMaterial>; 3],
    /// Authored door panel glbs per variant (hinge at origin, spans +X),
    /// spawned as an animated child of the door root entity. UV-unwrapped +
    /// COLOR_0, paired with the textured `*_door_material` below. Sources:
    /// `art/building/build_door.py`.
    pub(crate) hewn_door_panel_mesh: Handle<Mesh>,
    pub(crate) iron_door_panel_mesh: Handle<Mesh>,
    /// Textured door materials (base-white + plank/plate texture, COLOR_0
    /// tints the frame/braces/straps), one per variant.
    pub(crate) hewn_door_material: Handle<StandardMaterial>,
    pub(crate) iron_door_material: Handle<StandardMaterial>,
    /// Door placement ghost: closed panel + swing-arc indicator (procedural,
    /// shared by both variants; the ghost is a translucent preview).
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
    /// Cel-shaded wood material (hand-painted plank line-art, UV-mapped) for the
    /// wooden deployables: workbench, storage boxes, tool cupboard, torch.
    pub(crate) toon_wood_material: Handle<ToonMaterial>,
    /// Cel-shaded stone material (hand-painted cobble line-art, UV-mapped) for the
    /// crude furnace.
    pub(crate) toon_stone_material: Handle<ToonMaterial>,
    /// Cel-shaded fabric material (woven-quilt line-art, UV-mapped) for the
    /// sleeping bag bedroll. See [Toon / cel shading](../../../docs/toon-shading.md).
    pub(crate) toon_fabric_material: Handle<ToonMaterial>,
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

    /// Textured material for a building tier.
    pub(crate) fn building_material(
        &self,
        tier: crate::building::BuildingTier,
    ) -> Handle<StandardMaterial> {
        use crate::building::BuildingTier;
        let index = match tier {
            BuildingTier::Sticks => 0,
            BuildingTier::HewnWood => 1,
            BuildingTier::Stone => 2,
        };
        self.building_materials[index].clone()
    }

    /// Storage box mesh for a tier (1 = small, 2+ = large).
    pub(crate) fn storage_box_mesh(&self, tier: u8) -> Handle<Mesh> {
        if tier >= 2 {
            self.storage_box_large_mesh.clone()
        } else {
            self.storage_box_small_mesh.clone()
        }
    }

    /// Authored panel mesh for a door variant.
    pub(crate) fn door_panel_mesh(&self, variant: crate::items::DoorVariant) -> Handle<Mesh> {
        match variant {
            crate::items::DoorVariant::HewnLog => self.hewn_door_panel_mesh.clone(),
            crate::items::DoorVariant::Iron => self.iron_door_panel_mesh.clone(),
        }
    }

    /// Textured material for a door variant.
    pub(crate) fn door_material(
        &self,
        variant: crate::items::DoorVariant,
    ) -> Handle<StandardMaterial> {
        match variant {
            crate::items::DoorVariant::HewnLog => self.hewn_door_material.clone(),
            crate::items::DoorVariant::Iron => self.iron_door_material.clone(),
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
    mut toon_materials: ResMut<Assets<ToonMaterial>>,
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

    // Shared ore/vein rock-surface texture: a neutral grey cracked-stone detail
    // map (mean ~0.8) box-projected across every ore boulder + its chunks. Base
    // white * COLOR_0 (per-mineral) * this texture, the same trick as the bark.
    // Decoded sRGB with a CPU mip chain + repeat/aniso sampler. Source:
    // `art/ore/rock_master.png` -> `assets/textures/ore/rock.png`.
    let ore_rock_tex = {
        let rel = "textures/ore/rock.png";
        let bytes =
            embedded_bytes(rel).unwrap_or_else(|| panic!("embedded ore texture missing: {rel}"));
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
    // Ore/vein nodes: one authored glb per (type, depletion stage), each a single
    // mesh + single material slot with the per-mineral look on COLOR_0 (grey rock
    // body vs bright mineral chunks). Stage index = mesh 0/1/2 glb. See
    // `art/ore/build_ore.py`.
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
    commands.insert_resource(ResourceVisualAssets {
        coal_node_meshes: ore_stage_meshes("coal"),
        iron_node_meshes: ore_stage_meshes("iron"),
        sulfur_node_meshes: ore_stage_meshes("sulfur"),
        stone_vein_meshes: ore_stage_meshes("stone"),
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
        // One shared cel-shaded material for all four ores. The hand-painted rock
        // texture is shared; the per-mineral colour is the glb COLOR_0. params =
        // (cel band count, ambient floor, ink-edge strength, ink-edge width exp).
        // Harder cartoon: 3 bands + a strong dark silhouette edge. Ambient floor
        // lifted so the shadow bands read brighter (the stone was reading dark).
        ore_toon_material: toon_materials.add(ToonMaterial {
            detail: ore_rock_tex.clone(),
            params: Vec4::new(3.0, 0.42, 0.8, 2.2),
            tex_scale: 1.0, // ore glbs carry their own UVs; triplanar scale unused
            fade: 1.0,
        }),
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
            materials.add(hay_tall_grass_material(
                asset_server.load(embedded_asset_path("textures/tall_grass_1.png")),
            )),
            materials.add(hay_tall_grass_material(
                asset_server.load(embedded_asset_path("textures/tall_grass_2.png")),
            )),
            materials.add(hay_tall_grass_material(
                asset_server.load(embedded_asset_path("textures/tall_grass_3.png")),
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
            params: Vec4::new(3.0, 1.0, 0.55, 2.6),
            tex_scale: 1.0,
            fade: 1.0,
        }),
        birch_bark_material: toon_materials.add(ToonMaterial {
            detail: birch_bark_tex,
            params: Vec4::new(3.0, 1.0, 0.55, 2.6),
            tex_scale: 1.0,
            fade: 1.0,
        }),
        // Canopy: clean cel bands + a slightly wider ink edge so the leafy mass
        // reads with a drawn silhouette outline (the anime "sticker" look from the
        // references). The green rides the foliage detail texture; COLOR_0 layers
        // it dark (lower) -> light (crown).
        pine_foliage_material: toon_materials.add(ToonMaterial {
            detail: pine_foliage_tex,
            params: Vec4::new(3.0, 1.0, 0.7, 2.0),
            tex_scale: 1.0,
            fade: 1.0,
        }),
        birch_foliage_material: toon_materials.add(ToonMaterial {
            detail: birch_foliage_tex,
            params: Vec4::new(3.0, 1.0, 0.7, 2.0),
            tex_scale: 1.0,
            fade: 1.0,
        }),
        // Weathered dead bark: the same pine bark detail, but the dead-snag glb
        // carries a cool-grey COLOR_0 (set in build_tree.py) so `texture * COLOR_0`
        // reads grey and dead rather than like a live trunk. Cel-shaded like the
        // rest so a leafless snag still belongs to the family.
        dead_bark_material: toon_materials.add(ToonMaterial {
            detail: pine_bark_tex,
            params: Vec4::new(3.0, 1.0, 0.5, 2.6),
            tex_scale: 1.0,
            fade: 1.0,
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
    commands.insert_resource(DeployableVisualAssets {
        workbench_mesh: prim_mesh(&workbench_glb, 0),
        furnace_mesh: prim_mesh(&furnace_glb, 0),
        building_meshes,
        building_materials,
        hewn_door_panel_mesh: prim_mesh(&hewn_door_glb, 0),
        iron_door_panel_mesh: prim_mesh(&iron_door_glb, 0),
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
        sleeping_bag_mesh: prim_mesh(&sleeping_bag_glb, 0),
        storage_box_small_mesh: prim_mesh(&storage_box_small_glb, 0),
        storage_box_large_mesh: prim_mesh(&storage_box_large_glb, 0),
        torch_mesh: prim_mesh(&torch_glb, 0),
        tool_cupboard_mesh: prim_mesh(&tool_cupboard_glb, 0),
        // Per-surface cel-shaded materials for the deployables: hand-painted
        // wood / stone / fabric line-art mapped by the models' baked box-projected
        // UVs (tex_scale is the dead triplanar fallback, unused now the props
        // carry UVs). The deployables are mostly flat-faced boxes, so they run a
        // PUNCHIER cel than the rounded ore nodes (lower ambient floor for harder
        // plane-to-plane contrast, a full-strength + wider ink edge so every
        // beveled corner reads as a drawn outline). See docs/toon-shading.md
        // "flat-surface cel banding". Ore keeps its softer params.
        toon_wood_material: toon_materials.add(ToonMaterial {
            detail: deployable_wood_tex.clone(),
            params: Vec4::new(3.0, 0.46, 1.0, 1.4),
            tex_scale: 1.5,
            fade: 1.0,
        }),
        toon_stone_material: toon_materials.add(ToonMaterial {
            detail: deployable_stone_tex.clone(),
            params: Vec4::new(3.0, 0.46, 1.0, 1.4),
            tex_scale: 1.5,
            fade: 1.0,
        }),
        toon_fabric_material: toon_materials.add(ToonMaterial {
            detail: deployable_fabric_tex.clone(),
            params: Vec4::new(3.0, 0.46, 1.0, 1.4),
            tex_scale: 1.5,
            fade: 1.0,
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
