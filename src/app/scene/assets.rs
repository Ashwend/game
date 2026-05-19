use bevy::{
    post_process::dof::{DepthOfField, DepthOfFieldMode},
    prelude::*,
};

use super::{
    components::MainCamera,
    mesh::{
        COAL_ORE, IRON_ORE, SULFUR_ORE, impact_stone_shard_mesh, impact_wood_chip_mesh,
        low_poly_bag_mesh, low_poly_birch_tree_mesh, low_poly_dead_tree_mesh,
        low_poly_hatchet_mesh, low_poly_ore_node_mesh, low_poly_pickaxe_mesh,
        low_poly_pine_tree_mesh,
    },
};

use crate::app::{EYE_HEIGHT, PLAYER_VISUAL_CENTER_Y};

pub(crate) const REMOTE_PLAYER_COLOR: Color = Color::srgb(0.95, 0.61, 0.25);
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
    pub(crate) pine_tree_mesh: Handle<Mesh>,
    pub(crate) birch_tree_mesh: Handle<Mesh>,
    pub(crate) dead_tree_mesh: Handle<Mesh>,
    pub(crate) coal_material: Handle<StandardMaterial>,
    pub(crate) iron_material: Handle<StandardMaterial>,
    pub(crate) sulfur_material: Handle<StandardMaterial>,
    pub(crate) vertex_material: Handle<StandardMaterial>,
}

#[derive(Resource, Clone)]
pub(crate) struct ImpactEffectAssets {
    pub(crate) wood_chip_mesh: Handle<Mesh>,
    pub(crate) stone_shard_mesh: Handle<Mesh>,
    pub(crate) wood_chip_material: Handle<StandardMaterial>,
    pub(crate) stone_shard_material: Handle<StandardMaterial>,
}

pub(crate) fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
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
        Transform::from_xyz(0.0, EYE_HEIGHT, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            illuminance: 16_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(-3.0, 8.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.insert_resource(super::world::WorldSceneState::default());
    commands.insert_resource(PlayerVisualAssets {
        mesh: meshes.add(Capsule3d::new(0.35, 0.9)),
        remote_material: materials.add(REMOTE_PLAYER_COLOR),
    });
    commands.insert_resource(ItemVisualAssets {
        dropped_mesh: meshes.add(low_poly_bag_mesh()),
        held_bag_mesh: meshes.add(Cuboid::new(0.26, 0.22, 0.34)),
        held_hatchet_mesh: meshes.add(low_poly_hatchet_mesh()),
        held_pickaxe_mesh: meshes.add(low_poly_pickaxe_mesh()),
        dropped_material: materials.add(StandardMaterial {
            base_color: DROPPED_BAG_COLOR,
            perceptual_roughness: 0.95,
            ..default()
        }),
        held_bag_material: materials.add(StandardMaterial {
            base_color: HELD_BAG_COLOR,
            perceptual_roughness: 0.88,
            ..default()
        }),
        held_tool_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.92,
            ..default()
        }),
    });
    commands.insert_resource(ResourceVisualAssets {
        coal_node_mesh: meshes.add(low_poly_ore_node_mesh(COAL_ORE)),
        iron_node_mesh: meshes.add(low_poly_ore_node_mesh(IRON_ORE)),
        sulfur_node_mesh: meshes.add(low_poly_ore_node_mesh(SULFUR_ORE)),
        pine_tree_mesh: meshes.add(low_poly_pine_tree_mesh()),
        birch_tree_mesh: meshes.add(low_poly_birch_tree_mesh()),
        dead_tree_mesh: meshes.add(low_poly_dead_tree_mesh()),
        coal_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.98,
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
            perceptual_roughness: 0.62,
            ..default()
        }),
        vertex_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.98,
            ..default()
        }),
    });
    commands.insert_resource(ImpactEffectAssets {
        wood_chip_mesh: meshes.add(impact_wood_chip_mesh()),
        stone_shard_mesh: meshes.add(impact_stone_shard_mesh()),
        wood_chip_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.95,
            ..default()
        }),
        stone_shard_material: materials.add(StandardMaterial {
            base_color: VERTEX_MATERIAL_COLOR,
            perceptual_roughness: 0.88,
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
