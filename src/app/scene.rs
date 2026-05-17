use bevy::{
    asset::RenderAssetUsages,
    mesh::PrimitiveTopology,
    post_process::dof::{DepthOfField, DepthOfFieldMode},
    prelude::*,
};

use crate::{
    protocol::{ClientId, DroppedItemId, ResourceNodeId},
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
const VERTEX_MATERIAL_COLOR: Color = Color::WHITE;

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

#[derive(Component)]
pub(crate) struct NetworkPlayer {
    pub(crate) client_id: ClientId,
}

#[derive(Component)]
pub(crate) struct NetworkDroppedItem {
    pub(crate) id: DroppedItemId,
}

#[derive(Component)]
pub(crate) struct NetworkResourceNode {
    pub(crate) id: ResourceNodeId,
}

#[derive(Component)]
pub(crate) struct HeldItemVisual {
    pub(crate) item_id: String,
}

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

type MeshColor = [f32; 4];

const WOOD_DARK: MeshColor = [0.34, 0.21, 0.10, 1.0];
const WOOD_LIGHT: MeshColor = [0.56, 0.36, 0.18, 1.0];
const WOOD_MID: MeshColor = [0.44, 0.28, 0.14, 1.0];
const LEATHER_WRAP: MeshColor = [0.19, 0.12, 0.07, 1.0];
const IRON_BAND: MeshColor = [0.30, 0.30, 0.32, 1.0];
const STONE_DARK: MeshColor = [0.32, 0.34, 0.33, 1.0];
const STONE_LIGHT: MeshColor = [0.58, 0.61, 0.57, 1.0];
const STONE_EDGE: MeshColor = [0.74, 0.76, 0.72, 1.0];
const LEAF_PINE: MeshColor = [0.12, 0.34, 0.18, 1.0];
const LEAF_BIRCH: MeshColor = [0.38, 0.55, 0.26, 1.0];
const BIRCH_BARK: MeshColor = [0.78, 0.75, 0.65, 1.0];
const DEAD_WOOD: MeshColor = [0.33, 0.25, 0.17, 1.0];

#[derive(Clone, Copy)]
struct OreNodeStyle {
    base_color: MeshColor,
    accent_color: MeshColor,
    chunk_color: MeshColor,
    chunk_highlight: MeshColor,
    chunk_shape: OreChunkShape,
}

#[derive(Clone, Copy)]
enum OreChunkShape {
    Boulder,
    Crystal,
}

const COAL_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.26, 0.27, 0.28, 1.0],
    accent_color: [0.18, 0.19, 0.20, 1.0],
    chunk_color: [0.05, 0.05, 0.06, 1.0],
    chunk_highlight: [0.12, 0.12, 0.13, 1.0],
    chunk_shape: OreChunkShape::Boulder,
};

const IRON_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.52, 0.50, 0.46, 1.0],
    accent_color: [0.40, 0.38, 0.34, 1.0],
    chunk_color: [0.62, 0.30, 0.18, 1.0],
    chunk_highlight: [0.78, 0.42, 0.24, 1.0],
    chunk_shape: OreChunkShape::Boulder,
};

const SULFUR_ORE: OreNodeStyle = OreNodeStyle {
    base_color: [0.48, 0.46, 0.42, 1.0],
    accent_color: [0.36, 0.34, 0.30, 1.0],
    chunk_color: [0.96, 0.80, 0.18, 1.0],
    chunk_highlight: [1.00, 0.92, 0.36, 1.0],
    chunk_shape: OreChunkShape::Crystal,
};

#[derive(Default)]
struct LowPolyMeshBuilder {
    positions: Vec<[f32; 3]>,
    colors: Vec<MeshColor>,
    uvs: Vec<[f32; 2]>,
}

impl LowPolyMeshBuilder {
    fn push_triangle(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3], color: MeshColor) {
        self.positions.extend_from_slice(&[a, b, c]);
        self.colors.extend_from_slice(&[color, color, color]);
        self.uvs
            .extend_from_slice(&[[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]]);
    }

    fn push_triangle_away_from(
        &mut self,
        origin: [f32; 3],
        a: [f32; 3],
        b: [f32; 3],
        c: [f32; 3],
        color: MeshColor,
    ) {
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let normal = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        let centroid = [
            (a[0] + b[0] + c[0]) / 3.0 - origin[0],
            (a[1] + b[1] + c[1]) / 3.0 - origin[1],
            (a[2] + b[2] + c[2]) / 3.0 - origin[2],
        ];
        let dot = normal[0] * centroid[0] + normal[1] * centroid[1] + normal[2] * centroid[2];
        if dot < 0.0 {
            self.push_triangle(a, c, b, color);
        } else {
            self.push_triangle(a, b, c, color);
        }
    }

    fn add_box(&mut self, center: [f32; 3], half: [f32; 3], color: MeshColor) {
        let [cx, cy, cz] = center;
        let [hx, hy, hz] = half;
        let vertices = [
            [cx - hx, cy - hy, cz - hz],
            [cx + hx, cy - hy, cz - hz],
            [cx + hx, cy + hy, cz - hz],
            [cx - hx, cy + hy, cz - hz],
            [cx - hx, cy - hy, cz + hz],
            [cx + hx, cy - hy, cz + hz],
            [cx + hx, cy + hy, cz + hz],
            [cx - hx, cy + hy, cz + hz],
        ];
        let triangles = [
            (0, 2, 1),
            (0, 3, 2),
            (4, 5, 6),
            (4, 6, 7),
            (0, 7, 3),
            (0, 4, 7),
            (1, 2, 6),
            (1, 6, 5),
            (0, 1, 5),
            (0, 5, 4),
            (3, 7, 6),
            (3, 6, 2),
        ];
        for (a, b, c) in triangles {
            self.push_triangle_away_from(center, vertices[a], vertices[b], vertices[c], color);
        }
    }

    fn add_quad_prism(&mut self, points: [[f32; 2]; 4], half_depth: f32, color: MeshColor) {
        let origin = [
            (points[0][0] + points[1][0] + points[2][0] + points[3][0]) / 4.0,
            (points[0][1] + points[1][1] + points[2][1] + points[3][1]) / 4.0,
            0.0,
        ];
        let front = [
            [points[0][0], points[0][1], -half_depth],
            [points[1][0], points[1][1], -half_depth],
            [points[2][0], points[2][1], -half_depth],
            [points[3][0], points[3][1], -half_depth],
        ];
        let back = [
            [points[0][0], points[0][1], half_depth],
            [points[1][0], points[1][1], half_depth],
            [points[2][0], points[2][1], half_depth],
            [points[3][0], points[3][1], half_depth],
        ];
        self.push_triangle_away_from(origin, front[0], front[1], front[2], color);
        self.push_triangle_away_from(origin, front[0], front[2], front[3], color);
        self.push_triangle_away_from(origin, back[0], back[2], back[1], color);
        self.push_triangle_away_from(origin, back[0], back[3], back[2], color);
        for side in 0..4 {
            let next = (side + 1) % 4;
            self.push_triangle_away_from(origin, front[side], front[next], back[next], color);
            self.push_triangle_away_from(origin, front[side], back[next], back[side], color);
        }
    }

    fn add_tri_prism(&mut self, points: [[f32; 2]; 3], half_depth: f32, color: MeshColor) {
        let origin = [
            (points[0][0] + points[1][0] + points[2][0]) / 3.0,
            (points[0][1] + points[1][1] + points[2][1]) / 3.0,
            0.0,
        ];
        let front = [
            [points[0][0], points[0][1], -half_depth],
            [points[1][0], points[1][1], -half_depth],
            [points[2][0], points[2][1], -half_depth],
        ];
        let back = [
            [points[0][0], points[0][1], half_depth],
            [points[1][0], points[1][1], half_depth],
            [points[2][0], points[2][1], half_depth],
        ];
        self.push_triangle_away_from(origin, front[0], front[2], front[1], color);
        self.push_triangle_away_from(origin, back[0], back[1], back[2], color);
        for side in 0..3 {
            let next = (side + 1) % 3;
            self.push_triangle_away_from(origin, front[side], front[next], back[next], color);
            self.push_triangle_away_from(origin, front[side], back[next], back[side], color);
        }
    }

    fn add_cone(
        &mut self,
        base_y: f32,
        height: f32,
        radius: f32,
        segments: usize,
        color: MeshColor,
    ) {
        let apex = [0.0, base_y + height, 0.0];
        let origin = [0.0, base_y + height * 0.35, 0.0];
        let ring = (0..segments)
            .map(|index| {
                let angle = index as f32 / segments as f32 * std::f32::consts::TAU;
                [angle.cos() * radius, base_y, angle.sin() * radius]
            })
            .collect::<Vec<_>>();
        for index in 0..segments {
            let next = (index + 1) % segments;
            self.push_triangle_away_from(origin, apex, ring[index], ring[next], color);
        }
    }

    fn add_rock_lump(&mut self, center: [f32; 3], scale: [f32; 3], color: MeshColor) {
        let origin = [center[0], center[1] + 0.20 * scale[1], center[2]];
        let base = [
            [-0.62, 0.00, -0.18],
            [-0.34, 0.00, -0.50],
            [0.20, 0.00, -0.54],
            [0.58, 0.00, -0.18],
            [0.52, 0.00, 0.30],
            [0.05, 0.00, 0.54],
            [-0.48, 0.00, 0.32],
        ];
        let shoulder = [
            [-0.42, 0.22, -0.10],
            [-0.22, 0.30, -0.34],
            [0.18, 0.26, -0.36],
            [0.42, 0.20, -0.08],
            [0.34, 0.24, 0.22],
            [0.02, 0.32, 0.36],
            [-0.34, 0.25, 0.20],
        ];
        let peak = [0.02, 0.58, -0.02];

        let transform = |point: [f32; 3]| -> [f32; 3] {
            [
                center[0] + point[0] * scale[0],
                center[1] + point[1] * scale[1],
                center[2] + point[2] * scale[2],
            ]
        };

        for index in 0..base.len() {
            let next = (index + 1) % base.len();
            self.push_triangle_away_from(
                origin,
                transform(base[index]),
                transform(base[next]),
                transform(shoulder[next]),
                color,
            );
            self.push_triangle_away_from(
                origin,
                transform(base[index]),
                transform(shoulder[next]),
                transform(shoulder[index]),
                color,
            );
            self.push_triangle_away_from(
                origin,
                transform(peak),
                transform(shoulder[index]),
                transform(shoulder[next]),
                color,
            );
        }
    }

    fn add_crystal_cluster(
        &mut self,
        centre: [f32; 3],
        scale: [f32; 3],
        body: MeshColor,
        highlight: MeshColor,
    ) {
        let prongs: &[([f32; 3], [f32; 3], MeshColor)] = &[
            ([0.0, 0.0, 0.0], [0.0, 1.4, 0.0], body),
            ([0.6, -0.05, 0.1], [0.5, 1.1, 0.2], highlight),
            ([-0.55, -0.06, -0.1], [-0.55, 1.05, -0.1], body),
            ([0.18, -0.04, -0.55], [0.18, 1.0, -0.55], highlight),
        ];
        for (base, apex, color) in prongs {
            let bx = centre[0] + base[0] * scale[0];
            let by = centre[1] + base[1] * scale[1];
            let bz = centre[2] + base[2] * scale[2];
            let ax = centre[0] + apex[0] * scale[0] * 0.55;
            let ay = centre[1] + apex[1] * scale[1];
            let az = centre[2] + apex[2] * scale[2] * 0.55;
            let half = (scale[0] + scale[2]) * 0.12;
            let origin = [(bx + ax) * 0.5, (by + ay) * 0.5, (bz + az) * 0.5];
            let ring = [
                [bx - half, by, bz],
                [bx, by, bz + half],
                [bx + half, by, bz],
                [bx, by, bz - half],
            ];
            let apex_point = [ax, ay, az];
            for index in 0..4 {
                let next = (index + 1) % 4;
                self.push_triangle_away_from(origin, apex_point, ring[index], ring[next], *color);
            }
        }
    }

    fn add_octa_rock(&mut self, center: [f32; 3], scale: [f32; 3], color: MeshColor) {
        let [cx, cy, cz] = center;
        let [sx, sy, sz] = scale;
        let top = [cx, cy + sy, cz];
        let bottom = [cx, cy - sy * 0.82, cz];
        let ring = [
            [cx + sx * 0.95, cy + sy * 0.04, cz],
            [cx + sx * 0.42, cy - sy * 0.05, cz + sz * 0.72],
            [cx - sx * 0.24, cy + sy * 0.12, cz + sz * 0.88],
            [cx - sx * 0.90, cy - sy * 0.08, cz + sz * 0.14],
            [cx - sx * 0.46, cy + sy * 0.02, cz - sz * 0.78],
            [cx + sx * 0.38, cy - sy * 0.10, cz - sz * 0.82],
        ];
        for index in 0..ring.len() {
            let next = (index + 1) % ring.len();
            self.push_triangle_away_from(center, top, ring[index], ring[next], color);
            self.push_triangle_away_from(center, bottom, ring[next], ring[index], color);
        }
    }

    fn build(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
        .with_computed_flat_normals()
    }
}

fn low_poly_hatchet_mesh() -> Mesh {
    // Built in the same orientation convention as the pickaxe: the head
    // extends along mesh +X (which becomes world -Z, i.e. forward in the
    // first-person view, after the model's Y rotation). The mesh-Z axis is
    // the blade's thickness — kept thin so the blade reads as a blade rather
    // than a block from the side profile.
    let mut builder = LowPolyMeshBuilder::default();

    // Handle shaft (tapered look via two stacked boxes).
    builder.add_box([0.0, -0.06, 0.0], [0.024, 0.28, 0.024], WOOD_LIGHT);
    builder.add_box([0.0, -0.30, 0.0], [0.028, 0.06, 0.028], WOOD_MID);
    // Pommel knob.
    builder.add_box([0.0, -0.38, 0.0], [0.036, 0.030, 0.034], WOOD_DARK);
    // Leather grip wraps near the bottom of the shaft.
    builder.add_box([0.0, -0.20, 0.0], [0.031, 0.022, 0.031], LEATHER_WRAP);
    builder.add_box([0.0, -0.10, 0.0], [0.031, 0.014, 0.031], LEATHER_WRAP);
    // Iron band binding the head to the handle.
    builder.add_box([0.0, 0.17, 0.0], [0.054, 0.020, 0.038], IRON_BAND);
    // Wooden head saddle that the stone bit wraps around.
    builder.add_box([0.0, 0.22, 0.0], [0.050, 0.044, 0.040], WOOD_DARK);

    // Stone bit body — flared trapezoid in the mesh-XY plane. The half-depth
    // is small so the blade is a true blade in profile rather than a block.
    builder.add_quad_prism(
        [[0.04, 0.10], [0.22, 0.07], [0.32, 0.32], [0.04, 0.32]],
        0.020,
        STONE_LIGHT,
    );
    // Bright cutting edge along the leading curve of the bit. Sits slightly
    // proud of the body so the highlight catches the light during the swing.
    builder.add_tri_prism(
        [[0.22, 0.08], [0.36, 0.20], [0.30, 0.30]],
        0.013,
        STONE_EDGE,
    );
    // Beard — small downward hook at the front-bottom of the bit.
    builder.add_tri_prism(
        [[0.04, 0.10], [0.22, 0.05], [0.20, 0.10]],
        0.013,
        STONE_DARK,
    );
    // Upper horn — small triangular peak at the front-top, balances the beard.
    builder.add_tri_prism(
        [[0.04, 0.32], [0.22, 0.36], [0.28, 0.32]],
        0.013,
        STONE_DARK,
    );
    // Poll — short counterweight behind the eye (mesh -X), i.e. the back of
    // the head in the held view.
    builder.add_box([-0.07, 0.22, 0.0], [0.046, 0.036, 0.036], STONE_DARK);

    builder.build()
}

fn low_poly_pickaxe_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Longer, slightly heavier handle than the hatchet.
    builder.add_box([0.0, -0.08, 0.0], [0.026, 0.32, 0.026], WOOD_LIGHT);
    builder.add_box([0.0, -0.36, 0.0], [0.030, 0.060, 0.030], WOOD_MID);
    // Pommel knob.
    builder.add_box([0.0, -0.44, 0.0], [0.040, 0.030, 0.038], WOOD_DARK);
    // Leather grip wraps.
    builder.add_box([0.0, -0.24, 0.0], [0.033, 0.022, 0.033], LEATHER_WRAP);
    builder.add_box([0.0, -0.12, 0.0], [0.033, 0.014, 0.033], LEATHER_WRAP);
    builder.add_box([0.0, 0.00, 0.0], [0.033, 0.014, 0.033], LEATHER_WRAP);
    // Iron band binding the head.
    builder.add_box([0.0, 0.20, 0.0], [0.040, 0.020, 0.054], IRON_BAND);
    // Wooden head saddle that holds the stone pick.
    builder.add_box([0.0, 0.24, 0.0], [0.038, 0.040, 0.058], WOOD_DARK);
    // Stone cross bar.
    builder.add_box([0.0, 0.26, 0.0], [0.080, 0.030, 0.044], STONE_DARK);
    // Left pick spike — long tapered prong reaching out to the side.
    builder.add_quad_prism(
        [[-0.36, 0.26], [-0.08, 0.30], [-0.08, 0.22], [-0.34, 0.24]],
        0.034,
        STONE_LIGHT,
    );
    builder.add_tri_prism(
        [[-0.36, 0.26], [-0.42, 0.255], [-0.34, 0.24]],
        0.020,
        STONE_EDGE,
    );
    // Right pick spike — mirror of the left.
    builder.add_quad_prism(
        [[0.08, 0.30], [0.36, 0.26], [0.34, 0.24], [0.08, 0.22]],
        0.034,
        STONE_LIGHT,
    );
    builder.add_tri_prism(
        [[0.36, 0.26], [0.42, 0.255], [0.34, 0.24]],
        0.020,
        STONE_EDGE,
    );
    builder.build()
}

fn low_poly_ore_node_mesh(style: OreNodeStyle) -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Layered base rock mound — bigger central mass plus smaller flanking stones.
    builder.add_rock_lump([0.00, 0.00, 0.00], [1.00, 1.00, 1.00], style.base_color);
    builder.add_rock_lump([-0.32, 0.00, 0.18], [0.62, 0.74, 0.58], style.accent_color);
    builder.add_rock_lump([0.38, 0.00, -0.12], [0.54, 0.62, 0.52], style.accent_color);
    builder.add_rock_lump([0.04, 0.00, -0.38], [0.46, 0.52, 0.44], style.base_color);
    // Embedded ore chunks placed at varied heights/angles on top of the rocks.
    add_ore_chunks(&mut builder, style);
    builder.build()
}

fn add_ore_chunks(builder: &mut LowPolyMeshBuilder, style: OreNodeStyle) {
    let placements: &[([f32; 3], [f32; 3])] = &[
        ([0.06, 0.46, 0.08], [0.16, 0.18, 0.16]),
        ([-0.22, 0.32, -0.06], [0.12, 0.13, 0.11]),
        ([0.28, 0.30, 0.16], [0.13, 0.14, 0.12]),
        ([-0.18, 0.20, 0.34], [0.10, 0.12, 0.10]),
        ([0.22, 0.18, -0.30], [0.11, 0.13, 0.11]),
        ([-0.04, 0.10, 0.38], [0.09, 0.10, 0.09]),
    ];
    for (centre, scale) in placements {
        match style.chunk_shape {
            OreChunkShape::Boulder => {
                builder.add_octa_rock(*centre, *scale, style.chunk_color);
                builder.add_octa_rock(
                    [centre[0], centre[1] + scale[1] * 0.55, centre[2]],
                    [scale[0] * 0.45, scale[1] * 0.35, scale[2] * 0.45],
                    style.chunk_highlight,
                );
            }
            OreChunkShape::Crystal => {
                builder.add_crystal_cluster(
                    *centre,
                    *scale,
                    style.chunk_color,
                    style.chunk_highlight,
                );
            }
        }
    }
}

fn low_poly_pine_tree_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_box([0.0, 0.48, 0.0], [0.12, 0.48, 0.12], WOOD_DARK);
    builder.add_cone(0.58, 0.92, 0.74, 6, LEAF_PINE);
    builder.add_cone(1.02, 0.88, 0.58, 6, LEAF_PINE);
    builder.add_cone(1.42, 0.76, 0.42, 6, LEAF_PINE);
    builder.build()
}

fn low_poly_birch_tree_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_box([0.0, 0.55, 0.0], [0.105, 0.55, 0.105], BIRCH_BARK);
    builder.add_box([0.09, 0.36, 0.0], [0.018, 0.09, 0.112], STONE_DARK);
    builder.add_box([-0.08, 0.72, 0.0], [0.016, 0.08, 0.112], STONE_DARK);
    builder.add_octa_rock([0.0, 1.48, 0.0], [0.66, 0.48, 0.62], LEAF_BIRCH);
    builder.add_octa_rock([-0.26, 1.28, 0.08], [0.38, 0.30, 0.34], LEAF_BIRCH);
    builder.add_octa_rock([0.30, 1.30, -0.04], [0.34, 0.28, 0.34], LEAF_BIRCH);
    builder.build()
}

fn low_poly_dead_tree_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    builder.add_box([0.0, 0.74, 0.0], [0.13, 0.74, 0.13], DEAD_WOOD);
    builder.add_box([0.24, 1.10, 0.0], [0.25, 0.045, 0.045], DEAD_WOOD);
    builder.add_box([-0.20, 0.88, 0.0], [0.20, 0.040, 0.040], DEAD_WOOD);
    builder.add_box([0.0, 1.44, 0.0], [0.10, 0.12, 0.10], DEAD_WOOD);
    builder.build()
}

fn impact_wood_chip_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Splinter: a thin elongated box with a small darker cap to read at any angle.
    builder.add_box([0.0, 0.0, 0.0], [0.045, 0.012, 0.022], WOOD_LIGHT);
    builder.add_box([0.030, 0.0, 0.0], [0.015, 0.014, 0.018], WOOD_MID);
    builder.build()
}

fn impact_stone_shard_mesh() -> Mesh {
    let mut builder = LowPolyMeshBuilder::default();
    // Angular pebble: small octa rock plus a brighter cap face.
    builder.add_octa_rock([0.0, 0.0, 0.0], [0.05, 0.05, 0.05], STONE_DARK);
    builder.add_octa_rock([0.0, 0.022, 0.0], [0.028, 0.022, 0.028], STONE_EDGE);
    builder.build()
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
            resource_nodes: Vec::new(),
        };

        assert_eq!(snapshot.players[0].client_id, player.client_id);
        assert_eq!(snapshot.players[0].position, Vec3Net::new(1.0, 2.0, 3.0));
    }
}
