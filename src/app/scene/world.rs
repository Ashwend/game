use bevy::{light::NotShadowCaster, mesh::VertexAttributeValues, prelude::*};

use crate::{
    app::{
        scene::{DeployableVisualAssets, ResourceVisualAssets, ToonMaterial},
        state::{ClientRuntime, MenuState, Screen},
        systems::{
            insert_resource_node_material, resource_node_transform_at, resource_node_visual,
            tree_foliage_visual,
        },
    },
    resource_nodes::{resource_node_definition, spawn_resource_node},
    world::{BlockKind, WorldData, ruin_layout},
};

use super::{
    assets::WORLD_COLOR,
    components::WorldGeometry,
    grass::spawn_menu_grass,
    terrain::{TerrainMaterial, TerrainTextureAssets, build_terrain_material},
};

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

/// The ground plane's material. Live worlds get the biome-blended
/// [`TerrainMaterial`]; the menu backdrop and asset-less test apps fall back to
/// the original flat `StandardMaterial`.
enum GroundMaterial {
    Terrain(Handle<TerrainMaterial>),
    Flat(Handle<StandardMaterial>),
}

#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn apply_world_scene_system(
    mut commands: Commands,
    mut scene_state: ResMut<WorldSceneState>,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    resource_assets: Option<Res<ResourceVisualAssets>>,
    deployable_assets: Option<Res<DeployableVisualAssets>>,
    terrain_assets: Option<Res<TerrainTextureAssets>>,
    mut images: Option<ResMut<Assets<Image>>>,
    mut terrain_materials: Option<ResMut<Assets<TerrainMaterial>>>,
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
            // The menu floor is mostly hidden by props, grass, and the splash
            // depth-of-field blur, so it stays on the cheap flat material.
            let ground = GroundMaterial::Flat(flat_ground_material(&mut materials));
            spawn_world_geometry(
                &mut commands,
                &mut meshes,
                &mut materials,
                &backdrop,
                ground,
            );
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
                let ground = live_ground_material(
                    runtime.world_map_seed_dims.map(|(seed, _)| seed),
                    world.floor_size,
                    terrain_assets.as_deref(),
                    images.as_deref_mut(),
                    terrain_materials.as_deref_mut(),
                    &mut materials,
                );
                spawn_world_geometry(&mut commands, &mut meshes, &mut materials, world, ground);
                // Ruin structures are collision-only in `world.blocks` (tagged
                // `RuinMasonry`, skipped by the block renderer above); render
                // the real building-piece and prop meshes here from the shared
                // ruin layout (a pure function of the seed, identical to what
                // the server placed and the map draws).
                if let (Some(deployables), Some((seed, dims))) =
                    (deployable_assets.as_deref(), runtime.world_map_seed_dims)
                {
                    spawn_ruins(&mut commands, deployables, seed, dims);
                }
            }
        }
    }
    scene_state.applied = desired;
}

/// Spawn the visible burnt-house shells for a live world: one authored glb
/// per site (two primitives: charred timber + stone plinth/rubble, each on
/// its cel material). Collision comes from the `RuinMasonry` blocks in
/// `world.blocks`; this is purely visual. A shell is a single authored mesh,
/// so no two faces are coplanar by construction (the old building-piece
/// kitbash z-fought where foundations abutted). Tagged `WorldGeometry` so it
/// despawns on world change.
fn spawn_ruins(
    commands: &mut Commands,
    assets: &DeployableVisualAssets,
    seed: u64,
    dims: crate::world::ChunkDims,
) {
    for site in ruin_layout(seed, dims) {
        let base = Vec3::new(site.x, 0.0, site.z);
        let rotation = Quat::from_rotation_y(site.yaw());
        let (timber_mesh, masonry_mesh) = &assets.ruin_house_meshes[site.prefab.index()];
        // Charred timber: the wood cel material under a near-black COLOR_0
        // identity (the grain survives as faint ember-brown streaks).
        commands.spawn((
            Name::new(format!("Ruin {} timber", site.prefab.asset_stem())),
            WorldGeometry,
            Mesh3d(timber_mesh.clone()),
            MeshMaterial3d::<ToonMaterial>(assets.toon_wood_material.clone()),
            Transform::from_translation(base).with_rotation(rotation),
        ));
        // Stone plinth + rubble: the shared cel stone material.
        commands.spawn((
            Name::new(format!("Ruin {} masonry", site.prefab.asset_stem())),
            WorldGeometry,
            Mesh3d(masonry_mesh.clone()),
            MeshMaterial3d::<ToonMaterial>(assets.toon_stone_material.clone()),
            Transform::from_translation(base).with_rotation(rotation),
        ));
    }
}

/// The flat dark-green ground material (the look before biome texturing). Used
/// for the menu backdrop and as the fallback when the terrain assets aren't
/// available (e.g. scene unit tests with no render app).
fn flat_ground_material(materials: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: WORLD_COLOR,
        perceptual_roughness: 1.0,
        reflectance: 0.0,
        cull_mode: None,
        ..default()
    })
}

/// Pick the live-world ground material: the biome-blended terrain when the seed
/// and the terrain render assets are all present, otherwise the flat fallback.
fn live_ground_material(
    world_seed: Option<u64>,
    floor_size: f32,
    terrain_assets: Option<&TerrainTextureAssets>,
    images: Option<&mut Assets<Image>>,
    terrain_materials: Option<&mut Assets<TerrainMaterial>>,
    std_materials: &mut Assets<StandardMaterial>,
) -> GroundMaterial {
    match (world_seed, terrain_assets, images, terrain_materials) {
        (Some(seed), Some(textures), Some(images), Some(terrain_materials)) => {
            GroundMaterial::Terrain(build_terrain_material(
                seed,
                floor_size,
                textures,
                images,
                terrain_materials,
            ))
        }
        _ => GroundMaterial::Flat(flat_ground_material(std_materials)),
    }
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
        // Menu backdrop: no world seed, so its trees stay lush (dead is seed-derived).
        let Some(node) = spawn_resource_node(spawn, None) else {
            continue;
        };
        let Some(definition) = resource_node_definition(&node.definition_id) else {
            continue;
        };
        let (mesh, material) = resource_node_visual(assets, definition.model, node.id);
        let transform =
            resource_node_transform_at(node.id, node.position, node.yaw, definition.model);
        let mut node_command = commands.spawn((
            Name::new(format!("Menu Resource Node {}", node.id)),
            WorldGeometry,
            Mesh3d(mesh),
            transform,
        ));
        // Ore/vein nodes carry the cel-shaded `ToonMaterial`; trees keep their
        // `StandardMaterial` (distinct component types, attached after the spawn).
        insert_resource_node_material(&mut node_command, material);
        let entity = node_command.id();
        // Trees: attach the alpha-masked canopy as a child of the bark trunk, same
        // as the in-game spawn. The backdrop has no world seed so all trees are
        // live (never dead snags), and it's a close-up handful so no LOD is needed.
        // The child despawns with the `WorldGeometry` parent (recursive despawn).
        if let Some((foliage_mesh, foliage_material)) =
            tree_foliage_visual(assets, definition.model)
        {
            commands.entity(entity).with_children(|parent| {
                parent.spawn((
                    Mesh3d(foliage_mesh),
                    MeshMaterial3d(foliage_material),
                    Transform::default(),
                    Visibility::Visible,
                ));
            });
        }
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
    ground: GroundMaterial,
) {
    // A flat plane at the bottom of the world can never cast a visible shadow,
    // but without `NotShadowCaster` its ~33k triangles rasterise into every
    // directional-light cascade each frame. Receiving shadows is unaffected, so
    // the textured floor still takes tree/building shadows.
    let mut ground_entity = commands.spawn((
        Name::new("Authoritative Plane"),
        WorldGeometry,
        Mesh3d(meshes.add(build_ground_mesh(world.floor_size))),
        NotShadowCaster,
    ));
    // The biome-blended terrain material and the flat fallback are different
    // component types, so attach whichever this world resolved to.
    match ground {
        GroundMaterial::Terrain(handle) => ground_entity.insert(MeshMaterial3d(handle)),
        GroundMaterial::Flat(handle) => ground_entity.insert(MeshMaterial3d(handle)),
    };

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
        // Ruin masonry is collision-only here; the ruin rendering system
        // spawns the real building-piece and prop meshes at the site element
        // transforms, so skip drawing a plain grey cuboid for it.
        if block.kind == BlockKind::RuinMasonry {
            continue;
        }
        let size = block.size();
        let material = match block.kind {
            BlockKind::Stone => stone_material.clone(),
            BlockKind::Standard => block_materials[index % block_materials.len()].clone(),
            BlockKind::RuinMasonry => continue,
        };
        let name = match block.kind {
            BlockKind::Stone => format!("Stone Wall {}", index + 1),
            BlockKind::Standard => format!("Test Cube {}", index + 1),
            BlockKind::RuinMasonry => continue,
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
