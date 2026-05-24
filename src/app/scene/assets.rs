use bevy::{
    audio::SpatialListener,
    post_process::dof::{DepthOfField, DepthOfFieldMode},
    prelude::*,
};

use super::{
    components::MainCamera,
    mesh::{
        COAL_ORE, IRON_ORE, STONE_VEIN, SULFUR_ORE, impact_stone_shard_mesh, impact_wood_chip_mesh,
        low_poly_bag_mesh, low_poly_birch_tree_large_mesh, low_poly_birch_tree_medium_mesh,
        low_poly_birch_tree_small_mesh, low_poly_branch_pile_mesh, low_poly_hatchet_mesh,
        low_poly_hay_grass_mesh, low_poly_ore_node_mesh, low_poly_pickaxe_mesh,
        low_poly_pine_tree_large_mesh, low_poly_pine_tree_medium_mesh,
        low_poly_pine_tree_small_mesh, low_poly_player_mesh, low_poly_surface_stone_mesh,
    },
    sky::{initial_distance_fog, setup_sky},
};

use crate::app::{EYE_HEIGHT, PLAYER_VISUAL_CENTER_Y};

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
    pub(crate) held_hatchet_mesh: Handle<Mesh>,
    pub(crate) held_pickaxe_mesh: Handle<Mesh>,
    pub(crate) dropped_material: Handle<StandardMaterial>,
    pub(crate) held_bag_material: Handle<StandardMaterial>,
    pub(crate) held_tool_material: Handle<StandardMaterial>,
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
pub(crate) struct ImpactEffectAssets {
    pub(crate) wood_chip_mesh: Handle<Mesh>,
    pub(crate) stone_shard_mesh: Handle<Mesh>,
    pub(crate) wood_chip_material: Handle<StandardMaterial>,
    pub(crate) stone_shard_material: Handle<StandardMaterial>,
    /// Green-tinted material used for the `GrassBlades` particle burst.
    /// The mesh is reused from the stone shard so we don't pay for a
    /// second tiny mesh — the base-colour shift is enough to read as
    /// grass debris.
    pub(crate) grass_blade_material: Handle<StandardMaterial>,
}

pub(crate) fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
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
        Projection::from(PerspectiveProjection {
            fov: 65.0_f32.to_radians(),
            // Tight near/far for a ~30m playspace — improves depth precision
            // and keeps z-fighting away from on-screen geometry.
            near: 0.05,
            far: 200.0,
            ..default()
        }),
        Msaa::Off,
        menu_backdrop_depth_of_field(),
        // ~17cm between ears — keeps L/R panning natural for nearby spatial
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
    commands.insert_resource(ItemVisualAssets {
        dropped_mesh: meshes.add(low_poly_bag_mesh()),
        held_bag_mesh: meshes.add(Cuboid::new(0.26, 0.22, 0.34)),
        held_hatchet_mesh: meshes.add(low_poly_hatchet_mesh()),
        held_pickaxe_mesh: meshes.add(low_poly_pickaxe_mesh()),
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
        held_tool_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.92,
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
