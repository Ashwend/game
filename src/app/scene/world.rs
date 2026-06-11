use bevy::{light::NotShadowCaster, mesh::VertexAttributeValues, prelude::*};

use crate::{
    app::{
        scene::ResourceVisualAssets,
        state::{ClientRuntime, MenuState, Screen},
        systems::{resource_node_transform_at, resource_node_visual},
    },
    resources::{resource_node_definition, spawn_resource_node},
    world::{BlockKind, WorldData},
};

use super::{assets::WORLD_COLOR, components::WorldGeometry, grass::spawn_menu_grass};

pub(super) const STONE_WALL_COLOR: Color = Color::srgb(0.52, 0.53, 0.55);

/// What world geometry we last spawned into the scene. Compared against the
/// runtime's current selection in O(1) so we can skip the expensive respawn
/// when nothing changed, `WorldData` itself is never kept around for the
/// equality check.
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) enum WorldSceneSelection {
    #[default]
    None,
    /// Menu fallback, `WorldData::test_world()` is deterministic so it's
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

impl WorldSceneState {
    /// The live-world version currently spawned into the scene, if any. The
    /// loading-splash readiness gate uses this to confirm the geometry for the
    /// freshly joined world has actually been built before revealing it.
    pub(crate) fn applied_live_version(&self) -> Option<u64> {
        match self.applied {
            WorldSceneSelection::Live { version } => Some(version),
            _ => None,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_world_scene_system(
    mut commands: Commands,
    mut scene_state: ResMut<WorldSceneState>,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    resource_assets: Option<Res<ResourceVisualAssets>>,
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
            let backdrop = WorldData::menu_backdrop_world();
            spawn_world_geometry(&mut commands, &mut meshes, &mut materials, &backdrop);
            // The session path renders resource nodes from snapshots, but
            // the menu has no session, spawn them directly so the splash
            // camera has something interesting to look at.
            if let Some(assets) = resource_assets.as_deref() {
                spawn_menu_resource_nodes(&mut commands, assets, &backdrop);
            }
            // A swaying detail-grass carpet over the bare ground between props,
            // so the backdrop reads as a living woodland floor. GPU-instanced, so
            // it only needs the mesh assets, no material handle.
            spawn_menu_grass(&mut commands, &mut meshes);
        }
        WorldSceneSelection::Live { .. } => {
            if let Some(world) = runtime.world.as_ref() {
                spawn_world_geometry(&mut commands, &mut meshes, &mut materials, world);
            }
        }
    }
    scene_state.applied = desired;
}

/// Spawn static resource-node visuals as `WorldGeometry` so they live
/// alongside the menu floor and despawn when the player enters a real
/// session. Reuses the same mesh + material handles the session path
/// uses, so the menu and in-game art stay in lockstep.
fn spawn_menu_resource_nodes(
    commands: &mut Commands,
    assets: &ResourceVisualAssets,
    world: &WorldData,
) {
    for spawn in &world.resource_nodes {
        let Some(node) = spawn_resource_node(spawn) else {
            continue;
        };
        let Some(definition) = resource_node_definition(&node.definition_id) else {
            continue;
        };
        let (mesh, material) = resource_node_visual(assets, definition.model);
        let transform =
            resource_node_transform_at(node.id, node.position, node.yaw, definition.model);
        commands.spawn((
            Name::new(format!("Menu Resource Node {}", node.id)),
            WorldGeometry,
            Mesh3d(mesh),
            MeshMaterial3d(material),
            transform,
        ));
    }
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
        Mesh3d(meshes.add(build_ground_mesh(world.floor_size))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: WORLD_COLOR,
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            cull_mode: None,
            ..default()
        })),
        // A flat plane at the bottom of the world can never cast a
        // visible shadow, but without this marker its ~33k triangles
        // rasterise into every directional-light cascade each frame.
        // Receiving shadows is unaffected.
        NotShadowCaster,
    ));

    let block_materials = [
        materials.add(Color::srgb(0.46, 0.50, 0.48)),
        materials.add(Color::srgb(0.55, 0.48, 0.38)),
        materials.add(Color::srgb(0.36, 0.44, 0.55)),
        materials.add(Color::srgb(0.48, 0.40, 0.52)),
    ];
    let stone_material = materials.add(StandardMaterial {
        base_color: STONE_WALL_COLOR,
        perceptual_roughness: 0.95,
        reflectance: 0.1,
        ..default()
    });
    for (index, block) in world.blocks.iter().enumerate() {
        let size = block.size();
        let material = match block.kind {
            BlockKind::Stone => stone_material.clone(),
            BlockKind::Standard => block_materials[index % block_materials.len()].clone(),
        };
        let name = match block.kind {
            BlockKind::Stone => format!("Stone Wall {}", index + 1),
            BlockKind::Standard => format!("Test Cube {}", index + 1),
        };
        commands.spawn((
            Name::new(name),
            WorldGeometry,
            Mesh3d(meshes.add(Cuboid::new(size.x, size.y, size.z))),
            MeshMaterial3d(material),
            Transform::from_xyz(block.center.x, block.center.y, block.center.z),
        ));
    }
}

/// Plane mesh for the ground with per-vertex normals jittered by deterministic
/// multi-frequency noise. Positions are untouched so the floor stays flat for
/// movement/collision; only shading normals vary, which breaks up the otherwise
/// mirror-uniform specular highlight that made the ground look like wet glass
/// when the sun was low.
fn build_ground_mesh(floor_size: f32) -> Mesh {
    // Aim for ~2–4 m per quad so the normal jitter resolves across the size of
    // a typical highlight footprint without exploding vertex count on the
    // largest (576 m) maps.
    let subdivisions = 128;
    let mut mesh: Mesh = Plane3d::default()
        .mesh()
        .size(floor_size, floor_size)
        .subdivisions(subdivisions)
        .build();

    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION).cloned()
    else {
        return mesh;
    };
    if let Some(VertexAttributeValues::Float32x3(normals)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_NORMAL)
    {
        for (normal, position) in normals.iter_mut().zip(positions.iter()) {
            let x = position[0];
            let z = position[2];
            let nx = (x * 0.43).sin() * 0.06
                + (x * 1.27 + z * 0.91).sin() * 0.04
                + (x * 3.79 - z * 2.51).sin() * 0.02;
            let nz = (z * 0.51).cos() * 0.06
                + (z * 1.43 + x * 0.83).cos() * 0.04
                + (z * 3.97 + x * 2.71).cos() * 0.02;
            let tilted = Vec3::new(nx, 1.0, nz).normalize();
            *normal = [tilted.x, tilted.y, tilted.z];
        }
    }
    mesh
}
