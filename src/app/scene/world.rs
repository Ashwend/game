use bevy::prelude::*;

use crate::{
    app::state::{ClientRuntime, MenuState, Screen},
    world::WorldData,
};

use super::{assets::WORLD_COLOR, components::WorldGeometry};

/// What world geometry we last spawned into the scene. Compared against the
/// runtime's current selection in O(1) so we can skip the expensive respawn
/// when nothing changed — `WorldData` itself is never kept around for the
/// equality check.
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) enum WorldSceneSelection {
    #[default]
    None,
    /// Menu fallback — `WorldData::test_world()` is deterministic so it's
    /// fully identified by this variant.
    MenuBackdrop,
    /// A live world from a session. `version` ticks every time the runtime
    /// replaces `world`.
    Live { version: u64 },
}

#[derive(Resource, Default)]
pub(crate) struct WorldSceneState {
    applied: WorldSceneSelection,
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
    let desired = scene_selection(&runtime, menu.screen);
    if scene_state.applied == desired {
        return;
    }

    for entity in &geometry {
        commands.entity(entity).despawn();
    }

    match desired {
        WorldSceneSelection::None => {}
        WorldSceneSelection::MenuBackdrop => {
            spawn_world_geometry(
                &mut commands,
                &mut meshes,
                &mut materials,
                &WorldData::test_world(),
            );
        }
        WorldSceneSelection::Live { .. } => {
            if let Some(world) = runtime.world.as_ref() {
                spawn_world_geometry(&mut commands, &mut meshes, &mut materials, world);
            }
        }
    }
    scene_state.applied = desired;
}

fn scene_selection(runtime: &ClientRuntime, screen: Screen) -> WorldSceneSelection {
    if runtime.world.is_some() {
        WorldSceneSelection::Live {
            version: runtime.world_version,
        }
    } else if screen != Screen::InGame {
        WorldSceneSelection::MenuBackdrop
    } else {
        WorldSceneSelection::None
    }
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
