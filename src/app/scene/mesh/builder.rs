use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

pub(crate) type MeshColor = [f32; 4];

// Shared palette for low-poly props. Kept together so cross-prop visual
// consistency stays managed in one place.
pub(crate) const WOOD_LIGHT: MeshColor = [0.56, 0.36, 0.18, 1.0];
pub(crate) const WOOD_MID: MeshColor = [0.44, 0.28, 0.14, 1.0];
// The forged-iron head palette (IRON_HEAD / IRON_HEAD_DARK / IRON_EDGE) and the
// procedural deployable tones (WOOD_DARK / STONE_LIGHT / IRON_BAND /
// LEATHER_WRAP) used to live here. Those props are now authored Blender glbs
// (`art/items/iron_{hatchet,pickaxe}` tools, `art/items/{workbench_t1,crude_furnace}`
// structures), which bake the same linear tones into their COLOR_0 vertex
// colours, so the constants moved into the models.
pub(crate) const STONE_DARK: MeshColor = [0.32, 0.34, 0.33, 1.0];
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
        // Bottom cap, closes the underside so the cone is solid when seen
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

// ---------------------------------------------------------------------------
// Grass blades
//
// A tapered "blade" quad shared by the streamed detail grass (`scene::grass`,
// where it's instanced + wind-shaded) and the harvestable hay-grass node mesh
// (`mesh::crude`, a static clump). Unlike `LowPolyMeshBuilder`, blades use
// **upward (+Y) vertex normals** so they read as lit-from-above (catching sky
// light) rather than dark vertical walls, a per-vertex base→tip colour gradient,
// and `uv.x` carries an optional per-blade dither key (used by the detail-grass
// shader's distance fade; harmless for nodes that don't read it).
// ---------------------------------------------------------------------------

/// Base (dark) and tip (light) green for a blade, before the per-blade shade /
/// warmth tweaks. Tuned to sit at/just below the ground tone.
pub(crate) const GRASS_BLADE_BASE: [f32; 3] = [0.08, 0.18, 0.07];
pub(crate) const GRASS_BLADE_TIP: [f32; 3] = [0.19, 0.34, 0.13];

/// One grass blade. `base_color`/`tip_color` already include the shade/warmth
/// (their alpha is the sway weight, 0 base, 1 tip). `dither` is written to every
/// vertex's `uv.x` as a stable per-blade key.
pub(crate) struct GrassBlade {
    pub(crate) base: Vec2,
    pub(crate) yaw: f32,
    pub(crate) height: f32,
    pub(crate) half_width: f32,
    pub(crate) bend: Vec2,
    pub(crate) base_color: [f32; 4],
    pub(crate) tip_color: [f32; 4],
    pub(crate) dither: f32,
}

/// Base/tip blade colours for a darken-only `shade` (≤ 1.0, never glows brighter
/// than the ground) and a `warm` hue jitter in `[-1, 1]` (positive = warmer /
/// yellower). Alpha carries the sway weight (base 0, tip 1).
pub(crate) fn grass_blade_colors(shade: f32, warm: f32) -> ([f32; 4], [f32; 4]) {
    let tint = |rgb: [f32; 3], sway: f32| {
        [
            (rgb[0] * shade + warm * 0.05).clamp(0.0, 1.0),
            (rgb[1] * shade + warm * 0.01).clamp(0.0, 1.0),
            (rgb[2] * shade - warm * 0.03).clamp(0.0, 1.0),
            sway,
        ]
    };
    (tint(GRASS_BLADE_BASE, 0.0), tint(GRASS_BLADE_TIP, 1.0))
}

/// Indexed mesh builder for grass-blade clumps. Keeps the upward-normal /
/// gradient / dither convention out of `LowPolyMeshBuilder` (which is flat-normal
/// and per-face colour).
#[derive(Default)]
pub(crate) struct GrassBladeMesh {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl GrassBladeMesh {
    /// Append one tapered blade quad (base → near-point tip).
    pub(crate) fn push_blade(&mut self, blade: &GrassBlade) {
        let (s, c) = blade.yaw.sin_cos();
        // Width runs along the yaw-rotated local X axis (in the XZ plane).
        let ax = Vec2::new(c, s);
        let top_width = blade.half_width * 0.18;
        let base = blade.base;

        let bl = [
            base.x - ax.x * blade.half_width,
            0.0,
            base.y - ax.y * blade.half_width,
        ];
        let br = [
            base.x + ax.x * blade.half_width,
            0.0,
            base.y + ax.y * blade.half_width,
        ];
        let tcx = base.x + blade.bend.x;
        let tcz = base.y + blade.bend.y;
        let tl = [tcx - ax.x * top_width, blade.height, tcz - ax.y * top_width];
        let tr = [tcx + ax.x * top_width, blade.height, tcz + ax.y * top_width];

        let base_index = self.positions.len() as u32;
        self.positions.extend_from_slice(&[bl, br, tr, tl]);
        self.normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 4]);
        self.colors.extend_from_slice(&[
            blade.base_color,
            blade.base_color,
            blade.tip_color,
            blade.tip_color,
        ]);
        self.uvs.extend_from_slice(&[[blade.dither, 0.0]; 4]);
        self.indices.extend_from_slice(&[
            base_index,
            base_index + 1,
            base_index + 2,
            base_index,
            base_index + 2,
            base_index + 3,
        ]);
    }

    pub(crate) fn build(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grass_blade_bakes_sway_gradient_and_dither() {
        let (base_color, tip_color) = grass_blade_colors(0.9, 0.0);
        let mut b = GrassBladeMesh::default();
        b.push_blade(&GrassBlade {
            base: Vec2::new(1.0, 1.0),
            yaw: 0.3,
            height: 0.3,
            half_width: 0.03,
            bend: Vec2::ZERO,
            base_color,
            tip_color,
            dither: 0.42,
        });
        // Sway weight in colour alpha: base verts 0, tip verts 1.
        assert_eq!(b.colors[0][3], 0.0);
        assert_eq!(b.colors[2][3], 1.0);
        // Dither key in uv.x, identical on all four verts (whole-blade decision).
        assert!(b.uvs.iter().all(|uv| uv[0] == 0.42));
        // Tip sits above the base; normals point up.
        assert!(b.positions[2][1] > b.positions[0][1], "tip above base");
        assert!(b.normals.iter().all(|n| n[1] == 1.0), "upward normals");
    }

    #[test]
    fn grass_blade_colors_darken_only_and_warm_shifts_hue() {
        // Shade ≤ 1 never brightens past the base tip green.
        let (_, tip) = grass_blade_colors(1.0, 0.0);
        assert!(tip[1] <= GRASS_BLADE_TIP[1] + 1e-6);
        // Positive warmth pushes red up and blue down.
        let (_, warm_tip) = grass_blade_colors(1.0, 1.0);
        assert!(warm_tip[0] > tip[0]);
        assert!(warm_tip[2] < tip[2]);
    }
}
