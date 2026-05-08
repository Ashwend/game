use bevy::post_process::dof::{DepthOfField, DepthOfFieldMode};
use bevy::prelude::*;

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
        menu_backdrop_depth_of_field(),
        Transform::from_xyz(0.0, EYE_HEIGHT, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            illuminance: 16_000.0,
            shadows_enabled: true,
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
        dropped_mesh: meshes.add(Capsule3d::new(0.28, 0.34)),
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
        app::state::ClientRuntime,
        protocol::{PlayerState, Vec3Net, WorldSnapshot},
        world::WorldData,
    };

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
