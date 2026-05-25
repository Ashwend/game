//! Scene module: assets, components, mesh builders, and world geometry
//! application. The submodule layout is:
//!
//! - `components` — Bevy components that mark gameplay-relevant entities
//!   (`MainCamera`, `NetworkPlayer`, `NetworkDroppedItem`, …).
//! - `assets` — `Resource` definitions for shared meshes/materials plus the
//!   `setup_scene` startup system and small visual helpers.
//! - `mesh::*` — low-poly mesh builders for props (bag, tools, ore, trees,
//!   impact debris) and the shared color palette.
//! - `world` — `apply_world_scene_system`, `WorldSceneState`, and
//!   `WorldSceneSelection` (version-counter change detection).

mod assets;
mod components;
mod mesh;
mod sky;
mod world;

pub(crate) use assets::{
    ImpactEffectAssets, ItemVisualAssets, PlayerVisualAssets, ResourceVisualAssets,
    menu_backdrop_depth_of_field, player_visual_position, setup_scene,
};
pub(crate) use components::{
    HeldItemVisual, MainCamera, NetworkDroppedItem, NetworkPlayer, NetworkResourceNode,
    tree_mesh_height,
};
pub(crate) use mesh::PLAYER_HEAD_TOP_LOCAL_Y;
pub(crate) use sky::update_sky_system;
pub(crate) use world::apply_world_scene_system;
#[cfg(test)]
pub(crate) use {components::WorldGeometry, world::WorldSceneState};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{
            EYE_HEIGHT, PLAYER_VISUAL_CENTER_Y, state::ClientRuntime,
            systems::menu_backdrop_camera_system,
        },
        protocol::{PlayerState, Vec3Net, WorldSnapshot},
        world::WorldData,
    };
    use bevy::{
        anti_alias::taa::TemporalAntiAliasing, post_process::dof::DepthOfField, prelude::*,
    };

    use crate::app::state::{MenuState, Screen};

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

        // Sun and moon are two distinct directional lights; the sun
        // casts shadows and the moon does not.
        let lights: Vec<DirectionalLight> = world
            .query::<&DirectionalLight>()
            .iter(world)
            .cloned()
            .collect();
        assert_eq!(lights.len(), 2, "sun + moon directional lights");
        let shadow_casters = lights.iter().filter(|light| light.shadows_enabled).count();
        assert_eq!(shadow_casters, 1, "exactly the sun should cast shadows");
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
        let _ = EYE_HEIGHT;
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
                chat_bubble: None,
                inventory: None,
                crafting: None,
            }],
            dropped_items: Vec::new(),
            resource_nodes: Vec::new(),
        };

        assert_eq!(snapshot.players[0].client_id, player.client_id);
        assert_eq!(snapshot.players[0].position, Vec3Net::new(1.0, 2.0, 3.0));
    }
}
