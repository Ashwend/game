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

/// Base (lower) and tip (upper) green for a blade, before the per-blade shade /
/// warmth tweaks.
///
/// These are **linear** values (vertex colours are linear, unlike `Color::srgb`).
/// The ground is `Color::srgb(0.18, 0.34, 0.22)`, which is linear
/// `(0.027, 0.095, 0.040)`, so to read as *subtle terrain* rather than bright
/// spots, the grass must sit at that linear tone: base at/just below the ground,
/// tip only slightly above so the tips gently catch the eye. (Eyeballing these as
/// if they were sRGB made the grass ~5x brighter than the ground.)
pub(crate) const GRASS_BLADE_BASE: [f32; 3] = [0.022, 0.085, 0.040];
pub(crate) const GRASS_BLADE_TIP: [f32; 3] = [0.040, 0.130, 0.058];

/// One grass blade, shaped as a cubic-Bézier arch (root → leaned-over tip).
/// `base_color`/`tip_color` already include the shade/warmth (their alpha is the
/// sway weight, 0 base, 1 tip). `dither` is written to every vertex's `uv.x` as a
/// stable per-blade key.
pub(crate) struct GrassBlade {
    /// Root position in the mesh's local XZ plane.
    pub(crate) base: Vec2,
    /// Orientation of the blade's width axis (radians). Independent of `lean` so
    /// a blade can face any way regardless of which way it bows.
    pub(crate) yaw: f32,
    /// Blade length along its arc (m).
    pub(crate) height: f32,
    /// Half blade width at the root (m); tapers to a point at the tip.
    pub(crate) half_width: f32,
    /// Horizontal offset of the tip from the root (m): the direction and amount
    /// the blade leans over. Its magnitude must stay below `height`.
    pub(crate) lean: Vec2,
    /// Mid-blade arch as a fraction of `height`: how far the Bézier control
    /// points bow off the straight root→tip chord, giving the curl. 0 = straight.
    pub(crate) flex: f32,
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

/// Length-wise segments per blade. Each ring is one quad along the Bézier arc, so
/// a blade has `2 * (BLADE_SEGMENTS + 1)` verts and `2 * BLADE_SEGMENTS` tris.
/// Enough to read as a smooth curl without bloating the baked tile meshes.
const BLADE_SEGMENTS: usize = 5;

/// Cross-blade normal fan, radians. The two edge verts of each ring tilt their
/// normals out by `±` this around the blade's tangent, so a flat ribbon lights
/// like a rounded blade (the "rounded normal" trick from the Ghost-of-Tsushima /
/// Acerola grass talks). 0 = flat.
const BLADE_NORMAL_CURVE: f32 = 0.5;

/// How far each baked normal is pulled back toward straight-up (+Y), 0..1. A
/// vertical ribbon's true face normal is horizontal, which reads as a dark wall
/// under a top-down sun; biasing toward up keeps blades lit-from-above (bright)
/// while the fan + arch still give a soft rounded gradient. 0 = true face
/// normals (dark walls), 1 = pure up (flat, the pre-Bézier look).
const BLADE_NORMAL_UP_BIAS: f32 = 0.82;

/// Component-wise lerp of two RGBA colours (used to graduate colour + sway
/// weight up a multi-segment blade). Exact at the endpoints (`t == 0`/`1`).
fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// Normalize, falling back to `fallback` for a degenerate (near-zero) vector.
fn norm_or(v: Vec3, fallback: Vec3) -> Vec3 {
    v.try_normalize().unwrap_or(fallback)
}

fn cubic_bezier(t: f32, p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3) -> Vec3 {
    let u = 1.0 - t;
    u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3
}

fn cubic_bezier_tangent(t: f32, p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3) -> Vec3 {
    let u = 1.0 - t;
    3.0 * u * u * (p1 - p0) + 6.0 * u * t * (p2 - p1) + 3.0 * t * t * (p3 - p2)
}

/// Rodrigues rotation of `v` about unit axis `k` by `angle` radians.
fn rotate_about_axis(v: Vec3, k: Vec3, angle: f32) -> Vec3 {
    let (s, c) = angle.sin_cos();
    v * c + k.cross(v) * s + k * k.dot(v) * (1.0 - c)
}

/// Bias a normal back toward +Y (see [`BLADE_NORMAL_UP_BIAS`]) and emit it.
fn bias_up(n: Vec3) -> [f32; 3] {
    norm_or(n.lerp(Vec3::Y, BLADE_NORMAL_UP_BIAS), Vec3::Y).to_array()
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
    /// Append one cubic-Bézier blade: [`BLADE_SEGMENTS`] stacked quads sampled
    /// along the arc from a full-width root ring to a point tip. The blade leans
    /// over by `lean` and bows by `flex`; per-vertex normals are the ribbon's
    /// face normal, fanned across the width ([`BLADE_NORMAL_CURVE`]) for a rounded
    /// look and biased toward +Y ([`BLADE_NORMAL_UP_BIAS`]) so it stays lit from
    /// above. Colour and sway weight (vertex-colour alpha) graduate up the blade;
    /// the dither key is identical on every vert (whole-blade discard).
    pub(crate) fn push_blade(&mut self, blade: &GrassBlade) {
        let (s, c) = blade.yaw.sin_cos();
        // Width axis in the local XZ plane (the ribbon spans `±width_axis`).
        let width_axis = Vec3::new(c, 0.0, s);

        // Bézier control points in blade-local space (root at origin, +Y up).
        let p0 = Vec3::ZERO;
        let lean = Vec3::new(blade.lean.x, 0.0, blade.lean.y);
        // Tip height so the straight-line root→tip length stays ~`height`.
        let tip_y = (blade.height * blade.height - lean.length_squared())
            .max(1.0e-4)
            .sqrt();
        let p3 = Vec3::new(lean.x, tip_y, lean.y);
        // Arch axis: up-ish and perpendicular to the lean, so the blade curls
        // over its lean direction rather than twisting sideways.
        let lean_side = if blade.lean.length_squared() > 1.0e-6 {
            norm_or(Vec3::new(-blade.lean.y, 0.0, blade.lean.x), width_axis)
        } else {
            width_axis
        };
        let arch =
            norm_or(norm_or(p3, Vec3::Y).cross(lean_side), Vec3::Y) * (blade.height * blade.flex);
        let p1 = p0.lerp(p3, 0.33) + arch;
        let p2 = p0.lerp(p3, 0.66) + arch * 0.8;

        let base_index = self.positions.len() as u32;
        for ring in 0..=BLADE_SEGMENTS {
            let t = ring as f32 / BLADE_SEGMENTS as f32;
            let center = cubic_bezier(t, p0, p1, p2, p3);
            let tangent = norm_or(cubic_bezier_tangent(t, p0, p1, p2, p3), Vec3::Y);
            let half_width = blade.half_width * (1.0 - t * t);

            let left = center - width_axis * half_width;
            let right = center + width_axis * half_width;
            self.positions.extend_from_slice(&[
                [blade.base.x + left.x, left.y, blade.base.y + left.z],
                [blade.base.x + right.x, right.y, blade.base.y + right.z],
            ]);

            // Ribbon face normal, kept pointing up-ish, then fanned per edge.
            let mut face = norm_or(tangent.cross(width_axis), Vec3::Y);
            if face.y < 0.0 {
                face = -face;
            }
            self.normals.extend_from_slice(&[
                bias_up(rotate_about_axis(face, tangent, BLADE_NORMAL_CURVE)),
                bias_up(rotate_about_axis(face, tangent, -BLADE_NORMAL_CURVE)),
            ]);

            let color = lerp4(blade.base_color, blade.tip_color, t);
            self.colors.extend_from_slice(&[color, color]);
            // uv.x = stable per-blade dither key; uv.y = height fraction (unused
            // by the shader today, handy for debugging the gradient).
            self.uvs.extend_from_slice(&[[blade.dither, t]; 2]);
        }

        for seg in 0..BLADE_SEGMENTS as u32 {
            // Ring `seg` verts are (b, b+1) = (left, right); ring `seg + 1` are
            // (b+2, b+3). Two triangles span the quad: (lb, rb, rt), (lb, rt, lt).
            let b = base_index + seg * 2;
            self.indices
                .extend_from_slice(&[b, b + 1, b + 3, b, b + 3, b + 2]);
        }
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
            lean: Vec2::ZERO,
            flex: 0.0,
            base_color,
            tip_color,
            dither: 0.42,
        });
        // Sway weight in colour alpha: base ring 0, tip ring 1.
        assert_eq!(b.colors[0][3], 0.0);
        assert_eq!(b.colors.last().unwrap()[3], 1.0);
        // Dither key in uv.x, identical on every vert (whole-blade decision).
        assert!(b.uvs.iter().all(|uv| uv[0] == 0.42));
        // Tip ring sits above the base ring.
        let tip_y = b.positions.last().unwrap()[1];
        assert!(tip_y > b.positions[0][1], "tip above base");
        // Normals are unit length and biased upward (lit-from-above, no dark
        // walls), not the old hard-coded pure +Y.
        assert!(
            b.normals.iter().all(|n| {
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                (len - 1.0).abs() < 1.0e-3 && n[1] > 0.0
            }),
            "unit, upward-biased normals"
        );
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
