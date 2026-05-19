use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*};

pub(crate) type MeshColor = [f32; 4];

// Shared palette for low-poly props. Kept together so cross-prop visual
// consistency stays managed in one place.
pub(crate) const WOOD_DARK: MeshColor = [0.34, 0.21, 0.10, 1.0];
pub(crate) const WOOD_LIGHT: MeshColor = [0.56, 0.36, 0.18, 1.0];
pub(crate) const WOOD_MID: MeshColor = [0.44, 0.28, 0.14, 1.0];
pub(crate) const LEATHER_WRAP: MeshColor = [0.19, 0.12, 0.07, 1.0];
pub(crate) const IRON_BAND: MeshColor = [0.30, 0.30, 0.32, 1.0];
pub(crate) const STONE_DARK: MeshColor = [0.32, 0.34, 0.33, 1.0];
pub(crate) const STONE_LIGHT: MeshColor = [0.58, 0.61, 0.57, 1.0];
pub(crate) const STONE_EDGE: MeshColor = [0.74, 0.76, 0.72, 1.0];
pub(crate) const LEAF_PINE: MeshColor = [0.16, 0.36, 0.20, 1.0];
pub(crate) const LEAF_PINE_DARK: MeshColor = [0.08, 0.22, 0.11, 1.0];
pub(crate) const LEAF_PINE_LIGHT: MeshColor = [0.26, 0.50, 0.28, 1.0];
pub(crate) const LEAF_BIRCH: MeshColor = [0.42, 0.58, 0.28, 1.0];
pub(crate) const LEAF_BIRCH_DARK: MeshColor = [0.28, 0.42, 0.20, 1.0];
pub(crate) const LEAF_BIRCH_LIGHT: MeshColor = [0.60, 0.74, 0.36, 1.0];
pub(crate) const BIRCH_BARK: MeshColor = [0.85, 0.82, 0.74, 1.0];
pub(crate) const BIRCH_BARK_BAND: MeshColor = [0.18, 0.16, 0.14, 1.0];
pub(crate) const BARK_DARK: MeshColor = [0.20, 0.13, 0.06, 1.0];
pub(crate) const BARK_MID: MeshColor = [0.32, 0.20, 0.11, 1.0];
pub(crate) const DEAD_WOOD: MeshColor = [0.44, 0.34, 0.22, 1.0];
pub(crate) const DEAD_WOOD_DARK: MeshColor = [0.24, 0.17, 0.10, 1.0];

#[derive(Default)]
pub(crate) struct LowPolyMeshBuilder {
    positions: Vec<[f32; 3]>,
    colors: Vec<MeshColor>,
    uvs: Vec<[f32; 2]>,
}

impl LowPolyMeshBuilder {
    pub(crate) fn push_triangle(
        &mut self,
        a: [f32; 3],
        b: [f32; 3],
        c: [f32; 3],
        color: MeshColor,
    ) {
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

    pub(crate) fn add_box(&mut self, center: [f32; 3], half: [f32; 3], color: MeshColor) {
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

    pub(crate) fn add_quad_prism(
        &mut self,
        points: [[f32; 2]; 4],
        half_depth: f32,
        color: MeshColor,
    ) {
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

    pub(crate) fn add_tri_prism(
        &mut self,
        points: [[f32; 2]; 3],
        half_depth: f32,
        color: MeshColor,
    ) {
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

    pub(crate) fn add_cone(
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
        // Side faces (apex → ring).
        for index in 0..segments {
            let next = (index + 1) % segments;
            self.push_triangle_away_from(origin, apex, ring[index], ring[next], color);
        }
        // Bottom cap — closes the underside so the cone is solid when seen
        // from below (e.g. once a tree falls over). The `push_triangle_away`
        // helper picks the winding that points the normal outward from the
        // interior origin.
        let base_center = [0.0, base_y, 0.0];
        for index in 0..segments {
            let next = (index + 1) % segments;
            self.push_triangle_away_from(origin, base_center, ring[index], ring[next], color);
        }
    }

    pub(crate) fn add_rock_lump(&mut self, center: [f32; 3], scale: [f32; 3], color: MeshColor) {
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

    pub(crate) fn add_crystal_cluster(
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

    pub(crate) fn add_octa_rock(&mut self, center: [f32; 3], scale: [f32; 3], color: MeshColor) {
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

    pub(crate) fn build(self) -> Mesh {
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
