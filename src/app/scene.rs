use bevy::{
    asset::RenderAssetUsages,
    mesh::PrimitiveTopology,
    post_process::dof::{DepthOfField, DepthOfFieldMode},
    prelude::*,
};

use crate::{
    protocol::{ClientId, DroppedItemId},
    world::WorldData,
};

use super::{
    EYE_HEIGHT, PLAYER_VISUAL_CENTER_Y,
    state::{ClientRuntime, MenuState, Screen},
};

const REMOTE_PLAYER_COLOR: Color = Color::srgb(0.95, 0.61, 0.25);
const WORLD_COLOR: Color = Color::srgb(0.18, 0.34, 0.22);
const DROPPED_BAG_COLOR: Color = Color::srgb(0.42, 0.31, 0.18);
const HELD_BAG_COLOR: Color = Color::srgb(0.50, 0.38, 0.24);

#[derive(Resource, Default)]
pub(crate) struct WorldSceneState {
    applied: Option<WorldData>,
}

#[derive(Resource, Clone)]
pub(crate) struct PlayerVisualAssets {
    pub(crate) mesh: Handle<Mesh>,
    pub(crate) remote_material: Handle<StandardMaterial>,
}

#[derive(Resource, Clone)]
pub(crate) struct ItemVisualAssets {
    pub(crate) dropped_mesh: Handle<Mesh>,
    pub(crate) held_mesh: Handle<Mesh>,
    pub(crate) dropped_material: Handle<StandardMaterial>,
    pub(crate) held_material: Handle<StandardMaterial>,
}

#[derive(Component)]
pub(crate) struct NetworkPlayer {
    pub(crate) client_id: ClientId,
}

#[derive(Component)]
pub(crate) struct NetworkDroppedItem {
    pub(crate) id: DroppedItemId,
}

#[derive(Component)]
pub(crate) struct HeldItemVisual;

#[derive(Component)]
pub(crate) struct MainCamera;

#[derive(Component)]
pub(crate) struct WorldGeometry;

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

    commands.insert_resource(WorldSceneState::default());
    commands.insert_resource(PlayerVisualAssets {
        mesh: meshes.add(Capsule3d::new(0.35, 0.9)),
        remote_material: materials.add(REMOTE_PLAYER_COLOR),
    });
    commands.insert_resource(ItemVisualAssets {
        dropped_mesh: meshes.add(low_poly_bag_mesh()),
        held_mesh: meshes.add(Cuboid::new(0.26, 0.22, 0.34)),
        dropped_material: materials.add(StandardMaterial {
            base_color: DROPPED_BAG_COLOR,
            perceptual_roughness: 0.95,
            ..default()
        }),
        held_material: materials.add(StandardMaterial {
            base_color: HELD_BAG_COLOR,
            perceptual_roughness: 0.88,
            ..default()
        }),
    });
}

fn low_poly_bag_mesh() -> Mesh {
    let bottom = [
        [-0.07, -0.09, -0.05],
        [0.07, -0.09, -0.05],
        [0.09, -0.09, 0.02],
        [0.04, -0.09, 0.075],
        [-0.05, -0.09, 0.065],
        [-0.09, -0.09, 0.00],
    ];
    let belly = [
        [-0.10, -0.01, -0.075],
        [0.10, -0.01, -0.075],
        [0.12, -0.01, 0.02],
        [0.05, -0.01, 0.105],
        [-0.07, -0.01, 0.09],
        [-0.115, -0.01, -0.005],
    ];
    let shoulder = [
        [-0.08, 0.065, -0.06],
        [0.08, 0.065, -0.06],
        [0.095, 0.065, 0.015],
        [0.04, 0.065, 0.08],
        [-0.05, 0.065, 0.07],
        [-0.09, 0.065, -0.005],
    ];
    let neck = [
        [-0.032, 0.12, -0.022],
        [0.032, 0.12, -0.022],
        [0.04, 0.12, 0.012],
        [0.014, 0.12, 0.04],
        [-0.02, 0.12, 0.034],
        [-0.04, 0.12, 0.0],
    ];
    let top = [
        [-0.022, 0.145, -0.014],
        [0.022, 0.145, -0.014],
        [0.028, 0.145, 0.008],
        [0.01, 0.145, 0.026],
        [-0.014, 0.145, 0.022],
        [-0.028, 0.145, 0.0],
    ];

    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();

    for ring in [&bottom, &belly, &shoulder, &neck, &top] {
        for vertex in ring {
            positions.push(*vertex);
            uvs.push([0.0, 0.0]);
        }
    }

    for ring_index in 0..4 {
        let lower = ring_index * 6;
        let upper = (ring_index + 1) * 6;
        for side in 0..6 {
            let next = (side + 1) % 6;
            indices.extend_from_slice(&[
                (lower + side) as u32,
                (lower + next) as u32,
                (upper + side) as u32,
                (upper + side) as u32,
                (lower + next) as u32,
                (upper + next) as u32,
            ]);
        }
    }

    let bottom_center = positions.len() as u32;
    positions.push([0.0, -0.09, 0.0]);
    uvs.push([0.5, 0.0]);
    for side in 0..6 {
        indices.extend_from_slice(&[bottom_center, ((side + 1) % 6) as u32, side as u32]);
    }

    let top_center = positions.len() as u32;
    positions.push([0.0, 0.15, 0.006]);
    uvs.push([0.5, 1.0]);
    for side in 0..6 {
        let next = (side + 1) % 6;
        indices.extend_from_slice(&[top_center, 24 + side as u32, 24 + next as u32]);
    }

    let outward_indices = indices
        .chunks_exact(3)
        .flat_map(|triangle| [triangle[0], triangle[2], triangle[1]])
        .collect::<Vec<_>>();
    let flat_positions = outward_indices
        .iter()
        .map(|index| positions[*index as usize])
        .collect::<Vec<_>>();
    let flat_uvs = outward_indices
        .iter()
        .map(|index| uvs[*index as usize])
        .collect::<Vec<_>>();

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, flat_positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, flat_uvs)
    .with_computed_flat_normals()
}

pub(crate) fn apply_world_scene_system(
    mut commands: Commands,
    mut scene_state: ResMut<WorldSceneState>,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    geometry: Query<Entity, With<WorldGeometry>>,
) {
    let desired_world = scene_world(runtime.world.as_ref(), menu.screen);
    if scene_state.applied.as_ref() == desired_world.as_ref() {
        return;
    }

    for entity in &geometry {
        commands.entity(entity).despawn();
    }

    if let Some(world) = desired_world {
        spawn_world_geometry(&mut commands, &mut meshes, &mut materials, &world);
        scene_state.applied = Some(world);
    } else {
        scene_state.applied = None;
    }
}

fn scene_world(active_world: Option<&WorldData>, screen: Screen) -> Option<WorldData> {
    active_world
        .cloned()
        .or_else(|| (screen != Screen::InGame).then(WorldData::test_world))
}

fn spawn_world_geometry(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    world: &WorldData,
) {
    commands.spawn((
        Name::new("Authoritative Plane"),
        WorldGeometry,
        Mesh3d(
            meshes.add(
                Plane3d::default()
                    .mesh()
                    .size(world.floor_size, world.floor_size)
                    .subdivisions(16),
            ),
        ),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: WORLD_COLOR,
            perceptual_roughness: 0.9,
            cull_mode: None,
            ..default()
        })),
    ));

    let block_materials = [
        materials.add(Color::srgb(0.46, 0.50, 0.48)),
        materials.add(Color::srgb(0.55, 0.48, 0.38)),
        materials.add(Color::srgb(0.36, 0.44, 0.55)),
        materials.add(Color::srgb(0.48, 0.40, 0.52)),
    ];
    for (index, block) in world.blocks.iter().enumerate() {
        let size = block.size();
        commands.spawn((
            Name::new(format!("Test Cube {}", index + 1)),
            WorldGeometry,
            Mesh3d(meshes.add(Cuboid::new(size.x, size.y, size.z))),
            MeshMaterial3d(block_materials[index % block_materials.len()].clone()),
            Transform::from_xyz(block.center.x, block.center.y, block.center.z),
        ));
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{state::ClientRuntime, systems::menu_backdrop_camera_system},
        protocol::{PlayerState, Vec3Net, WorldSnapshot},
        world::WorldData,
    };
    use bevy::anti_alias::taa::TemporalAntiAliasing;

    fn app_with_scene_resources() -> App {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>();
        app.init_resource::<Assets<StandardMaterial>>();
        app
    }

    #[test]
    fn setup_scene_creates_camera_light_and_assets() {
        let mut app = app_with_scene_resources();
        app.add_systems(Startup, setup_scene);
        app.update();

        assert!(app.world().contains_resource::<WorldSceneState>());
        assert!(app.world().contains_resource::<PlayerVisualAssets>());
        let camera_count = {
            let world = app.world_mut();
            let mut query = world.query::<&MainCamera>();
            query.iter(world).count()
        };
        assert_eq!(camera_count, 1);

        let world = app.world_mut();
        let msaa = world
            .query_filtered::<&Msaa, With<MainCamera>>()
            .single(world)
            .expect("main camera should start with menu-compatible msaa");
        assert_eq!(*msaa, Msaa::Off);
        let temporal_aa_count = world
            .query_filtered::<&TemporalAntiAliasing, With<MainCamera>>()
            .iter(world)
            .count();
        assert_eq!(temporal_aa_count, 0);

        let sun = world
            .query::<&DirectionalLight>()
            .single(world)
            .expect("sun should exist");
        assert!(!sun.shadows_enabled);
    }

    #[test]
    fn gameplay_camera_rendering_avoids_temporal_double_image_artifacts() {
        let mut app = app_with_scene_resources();
        app.insert_resource(MenuState {
            screen: Screen::InGame,
            ..Default::default()
        });
        app.add_systems(Startup, setup_scene);
        app.add_systems(Update, menu_backdrop_camera_system);

        app.update();

        let world = app.world_mut();
        let msaa = world
            .query_filtered::<&Msaa, With<MainCamera>>()
            .single(world)
            .expect("main camera should exist");
        assert_eq!(*msaa, Msaa::Sample4);

        let depth_of_field_count = world
            .query_filtered::<&DepthOfField, With<MainCamera>>()
            .iter(world)
            .count();
        assert_eq!(depth_of_field_count, 0);

        let temporal_aa_count = world
            .query_filtered::<&TemporalAntiAliasing, With<MainCamera>>()
            .iter(world)
            .count();
        assert_eq!(temporal_aa_count, 0);
    }

    #[test]
    fn applying_world_scene_spawns_and_clears_geometry() {
        let mut app = app_with_scene_resources();
        app.insert_resource(WorldSceneState::default());
        app.insert_resource(MenuState::default());
        app.insert_resource(ClientRuntime {
            world: Some(WorldData::test_world()),
            ..Default::default()
        });
        app.add_systems(Update, apply_world_scene_system);
        app.update();

        let geometry_count = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<WorldGeometry>>();
            query.iter(world).count()
        };
        assert!(geometry_count > 0);

        app.world_mut().resource_mut::<ClientRuntime>().world = None;
        app.world_mut().resource_mut::<MenuState>().screen = Screen::InGame;
        app.update();

        let geometry_count = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<WorldGeometry>>();
            query.iter(world).count()
        };
        assert_eq!(geometry_count, 0);
    }

    #[test]
    fn menu_without_active_world_uses_test_world_backdrop() {
        let mut app = app_with_scene_resources();
        app.insert_resource(WorldSceneState::default());
        app.insert_resource(MenuState::default());
        app.insert_resource(ClientRuntime::default());
        app.add_systems(Update, apply_world_scene_system);
        app.update();

        let geometry_count = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<WorldGeometry>>();
            query.iter(world).count()
        };
        assert!(geometry_count > 0);
    }

    #[test]
    fn player_visuals_are_offset_from_feet() {
        let feet = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(
            player_visual_position(feet),
            feet + Vec3::Y * PLAYER_VISUAL_CENTER_Y
        );
    }

    #[test]
    fn network_marker_components_store_client_ids() {
        let player = NetworkPlayer { client_id: 7 };
        let snapshot = WorldSnapshot {
            tick: 1,
            players: vec![PlayerState {
                client_id: player.client_id,
                steam_id: 7,
                name: "Remote".to_owned(),
                position: Vec3Net::new(1.0, 2.0, 3.0),
                velocity: Vec3Net::ZERO,
                yaw: 1.0,
                pitch: 0.0,
                health: 100.0,
                grounded: true,
                last_processed_input: 0,
                is_admin: false,
                inventory: Default::default(),
            }],
            dropped_items: Vec::new(),
        };

        assert_eq!(snapshot.players[0].client_id, player.client_id);
        assert_eq!(snapshot.players[0].position, Vec3Net::new(1.0, 2.0, 3.0));
    }
}
