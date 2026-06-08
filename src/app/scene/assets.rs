use bevy::{
    audio::SpatialListener,
    core_pipeline::tonemapping::Tonemapping,
    gltf::GltfAssetLabel,
    light::AtmosphereEnvironmentMapLight,
    pbr::{Atmosphere, AtmosphereSettings, ScatteringMedium},
    post_process::dof::{DepthOfField, DepthOfFieldMode},
    prelude::*,
    render::view::{Hdr, NoIndirectDrawing},
};

use super::{
    components::MainCamera,
    grass::{GrassMaterial, GrassMaterialHandle, grass_material},
    mesh::{
        COAL_ORE, IRON_ORE, STONE_VEIN, SULFUR_ORE, impact_stone_shard_mesh, impact_wood_chip_mesh,
        low_poly_bag_mesh, low_poly_birch_tree_large_lod_mesh, low_poly_birch_tree_large_mesh,
        low_poly_birch_tree_medium_lod_mesh, low_poly_birch_tree_medium_mesh,
        low_poly_birch_tree_small_lod_mesh, low_poly_birch_tree_small_mesh,
        low_poly_branch_pile_mesh, low_poly_hay_grass_mesh, low_poly_ore_node_mesh,
        low_poly_pine_tree_large_lod_mesh, low_poly_pine_tree_large_mesh,
        low_poly_pine_tree_medium_lod_mesh, low_poly_pine_tree_medium_mesh,
        low_poly_pine_tree_small_lod_mesh, low_poly_pine_tree_small_mesh, low_poly_player_mesh,
        low_poly_surface_stone_mesh,
    },
    sky::{initial_distance_fog, setup_sky},
};

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
    pub(crate) mesh: Handle<Mesh>,
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
}

#[derive(Resource, Clone)]
pub(crate) struct ResourceVisualAssets {
    pub(crate) coal_node_mesh: Handle<Mesh>,
    pub(crate) iron_node_mesh: Handle<Mesh>,
    pub(crate) sulfur_node_mesh: Handle<Mesh>,
    pub(crate) stone_vein_mesh: Handle<Mesh>,
    pub(crate) pine_tree_small_mesh: Handle<Mesh>,
    pub(crate) pine_tree_medium_mesh: Handle<Mesh>,
    pub(crate) pine_tree_large_mesh: Handle<Mesh>,
    pub(crate) birch_tree_small_mesh: Handle<Mesh>,
    pub(crate) birch_tree_medium_mesh: Handle<Mesh>,
    pub(crate) birch_tree_large_mesh: Handle<Mesh>,
    /// Low-poly distance LOD variants of the trees, swapped in past the LOD
    /// distance via `VisibilityRange` hard switch (see the resource-node spawn).
    pub(crate) pine_tree_small_lod_mesh: Handle<Mesh>,
    pub(crate) pine_tree_medium_lod_mesh: Handle<Mesh>,
    pub(crate) pine_tree_large_lod_mesh: Handle<Mesh>,
    pub(crate) birch_tree_small_lod_mesh: Handle<Mesh>,
    pub(crate) birch_tree_medium_lod_mesh: Handle<Mesh>,
    pub(crate) birch_tree_large_lod_mesh: Handle<Mesh>,
    pub(crate) surface_stone_mesh: Handle<Mesh>,
    pub(crate) branch_pile_mesh: Handle<Mesh>,
    pub(crate) hay_grass_mesh: Handle<Mesh>,
    pub(crate) coal_material: Handle<StandardMaterial>,
    pub(crate) iron_material: Handle<StandardMaterial>,
    pub(crate) sulfur_material: Handle<StandardMaterial>,
    pub(crate) stone_vein_material: Handle<StandardMaterial>,
    pub(crate) vertex_material: Handle<StandardMaterial>,
}

#[derive(Resource, Clone)]
pub(crate) struct DeployableVisualAssets {
    pub(crate) workbench_mesh: Handle<Mesh>,
    pub(crate) furnace_mesh: Handle<Mesh>,
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

#[derive(Resource, Clone)]
pub(crate) struct ImpactEffectAssets {
    pub(crate) wood_chip_mesh: Handle<Mesh>,
    pub(crate) stone_shard_mesh: Handle<Mesh>,
    pub(crate) wood_chip_material: Handle<StandardMaterial>,
    pub(crate) stone_shard_material: Handle<StandardMaterial>,
    /// Green-tinted material used for the `GrassBlades` particle burst.
    /// The mesh is reused from the stone shard so we don't pay for a
    /// second tiny mesh, the base-colour shift is enough to read as
    /// grass debris.
    pub(crate) grass_blade_material: Handle<StandardMaterial>,
}

pub(crate) fn setup_scene(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut scattering_media: ResMut<Assets<ScatteringMedium>>,
    mut grass_materials: ResMut<Assets<GrassMaterial>>,
) {
    // The one shared grass material (wind + distance-fade shader), created
    // eagerly so both the streamed detail grass and the harvestable hay-grass
    // node reference the same instance and sway in unison.
    commands.insert_resource(GrassMaterialHandle(grass_materials.add(grass_material())));
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
        Tonemapping::TonyMcMapface,
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
            // Tight near/far. The far plane sits just past the daylight
            // fog horizon (~140 m peak) so the perimeter walls of a Large
            // 9×9 chunk world (288 m from centre) never poke through the
            // fog wall. Keeping it tight also improves depth precision
            // and keeps z-fighting away from on-screen geometry.
            near: 0.05,
            far: 160.0,
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
        mesh: meshes.add(low_poly_player_mesh()),
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
    });
    commands.insert_resource(ResourceVisualAssets {
        coal_node_mesh: meshes.add(low_poly_ore_node_mesh(COAL_ORE)),
        iron_node_mesh: meshes.add(low_poly_ore_node_mesh(IRON_ORE)),
        sulfur_node_mesh: meshes.add(low_poly_ore_node_mesh(SULFUR_ORE)),
        stone_vein_mesh: meshes.add(low_poly_ore_node_mesh(STONE_VEIN)),
        pine_tree_small_mesh: meshes.add(low_poly_pine_tree_small_mesh()),
        pine_tree_medium_mesh: meshes.add(low_poly_pine_tree_medium_mesh()),
        pine_tree_large_mesh: meshes.add(low_poly_pine_tree_large_mesh()),
        birch_tree_small_mesh: meshes.add(low_poly_birch_tree_small_mesh()),
        birch_tree_medium_mesh: meshes.add(low_poly_birch_tree_medium_mesh()),
        birch_tree_large_mesh: meshes.add(low_poly_birch_tree_large_mesh()),
        pine_tree_small_lod_mesh: meshes.add(low_poly_pine_tree_small_lod_mesh()),
        pine_tree_medium_lod_mesh: meshes.add(low_poly_pine_tree_medium_lod_mesh()),
        pine_tree_large_lod_mesh: meshes.add(low_poly_pine_tree_large_lod_mesh()),
        birch_tree_small_lod_mesh: meshes.add(low_poly_birch_tree_small_lod_mesh()),
        birch_tree_medium_lod_mesh: meshes.add(low_poly_birch_tree_medium_lod_mesh()),
        birch_tree_large_lod_mesh: meshes.add(low_poly_birch_tree_large_lod_mesh()),
        surface_stone_mesh: meshes.add(low_poly_surface_stone_mesh()),
        branch_pile_mesh: meshes.add(low_poly_branch_pile_mesh()),
        hay_grass_mesh: meshes.add(low_poly_hay_grass_mesh()),
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
    });
    // Placed structures are authored Blender glbs matching their inventory icons
    // (a splay-legged wooden bench, a cobblestone furnace with an arched glowing
    // mouth). Like the tools, each look is carried by the model's COLOR_0 vertex
    // colours; only the mesh primitive is loaded here, the shared `material` below
    // stays base-white so those vertex colours show through, exactly as the
    // procedural trees/ore nodes do. Sources:
    // `art/items/{workbench_t1,crude_furnace}/*.blend`.
    let workbench_glb = embedded_asset_path("items/workbench_t1/model.glb");
    let furnace_glb = embedded_asset_path("items/crude_furnace/model.glb");
    commands.insert_resource(DeployableVisualAssets {
        workbench_mesh: prim_mesh(&workbench_glb, 0),
        furnace_mesh: prim_mesh(&furnace_glb, 0),
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
    });
    commands.insert_resource(FurnaceFireAssets {
        // A small round puff. The fire is built from a dense stream of these
        // rising and fading; additive blending fuses the overlap into a soft
        // glowing flame body, so no single particle reads as a hard shape.
        flame_mesh: meshes.add(Sphere::new(0.07)),
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
