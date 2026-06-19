use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

pub(crate) type MeshColor = [f32; 4];

// Shared palette for low-poly props. Kept together so cross-prop visual
// consistency stays managed in one place.
//
// These are **linear** albedos (vertex colours bypass the sRGB decode that
// `Color::srgb` applies). The terrain anchor is the ground at linear
// (0.027, 0.095, 0.040), so prop values must live in the same range to sit
// in the scene rather than glow against it: rock ~0.08-0.25, bark ~0.03-0.09,
// foliage green channel ~0.06-0.22. Picking these as if they were sRGB makes
// every prop render 2-5x brighter than intended (chalk-white rocks, pastel
// trees); see GRASS_BLADE_BASE below, where the same lesson was learned first.
pub(crate) const WOOD_LIGHT: MeshColor = [0.260, 0.110, 0.035, 1.0];
pub(crate) const WOOD_MID: MeshColor = [0.160, 0.065, 0.020, 1.0];
// The forged-iron head palette (IRON_HEAD / IRON_HEAD_DARK / IRON_EDGE) and the
// procedural deployable tones (WOOD_DARK / STONE_LIGHT / IRON_BAND /
// LEATHER_WRAP) used to live here. Those props are now authored Blender glbs
// (`art/items/iron_{hatchet,pickaxe}` tools, `art/items/{workbench_t1,crude_furnace}`
// structures), which bake the same linear tones into their COLOR_0 vertex
// colours, so the constants moved into the models.
pub(crate) const STONE_DARK: MeshColor = [0.080, 0.090, 0.085, 1.0];
pub(crate) const STONE_EDGE: MeshColor = [0.380, 0.400, 0.360, 1.0];
// Foliage greens for the procedural LOD stand-ins (the full-detail trees are now
// textured glbs). Retuned toward the muted needle/leaf texture midtones so the
// 80 m `VisibilityRange` switch from textured glb to flat LOD doesn't flip the
// canopy brightness. `*_DARK` (lower layers) through `*_LIGHT` (crown) keeps the
// shaded depth the textures carry.
pub(crate) const LEAF_PINE: MeshColor = [0.030, 0.110, 0.042, 1.0];
pub(crate) const LEAF_PINE_DARK: MeshColor = [0.016, 0.062, 0.026, 1.0];
pub(crate) const LEAF_PINE_LIGHT: MeshColor = [0.055, 0.160, 0.060, 1.0];
pub(crate) const LEAF_BIRCH: MeshColor = [0.110, 0.215, 0.050, 1.0];
pub(crate) const LEAF_BIRCH_LIGHT: MeshColor = [0.210, 0.350, 0.082, 1.0];
pub(crate) const BIRCH_BARK: MeshColor = [0.500, 0.480, 0.420, 1.0];
pub(crate) const BARK_DARK: MeshColor = [0.040, 0.022, 0.010, 1.0];

/// Scale a colour's RGB by `factor`, leaving alpha untouched. Used to bake
/// cheap ambient-occlusion-ish darkening (undersides, ground-contact bands)
/// into the flat-shaded props without growing the palette.
pub(crate) fn scale_rgb(color: MeshColor, factor: f32) -> MeshColor {
    [
        (color[0] * factor).clamp(0.0, 1.0),
        (color[1] * factor).clamp(0.0, 1.0),
        (color[2] * factor).clamp(0.0, 1.0),
        color[3],
    ]
}

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
        self.add_cone_at([0.0, base_y, 0.0], height, radius, segments, color);
    }

    /// `add_cone` with an explicit base-centre, so foliage layers can sit
    /// slightly off the trunk axis (real conifers aren't lathe-symmetric).
    pub(crate) fn add_cone_at(
        &mut self,
        base_center: [f32; 3],
        height: f32,
        radius: f32,
        segments: usize,
        color: MeshColor,
    ) {
        let [bx, base_y, bz] = base_center;
        let apex = [bx, base_y + height, bz];
        let origin = [bx, base_y + height * 0.35, bz];
        let ring = (0..segments)
            .map(|index| {
                let angle = index as f32 / segments as f32 * std::f32::consts::TAU;
                [bx + angle.cos() * radius, base_y, bz + angle.sin() * radius]
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
        // interior origin. Darkened: a foliage layer's underside is in its
        // own shadow, and the visible rim of the cap is what sells the
        // canopy as dense from eye level.
        let cap_color = scale_rgb(color, 0.45);
        for index in 0..segments {
            let next = (index + 1) % segments;
            self.push_triangle_away_from(origin, base_center, ring[index], ring[next], cap_color);
        }
    }

    /// An `add_box` rotated by `yaw` (about Y) then `pitch` (about its local
    /// length axis' perpendicular, tipping the +X end up/down). Lets sticks
    /// and stub branches cross and lean instead of stacking axis-aligned.
    pub(crate) fn add_box_oriented(
        &mut self,
        center: [f32; 3],
        half: [f32; 3],
        yaw: f32,
        pitch: f32,
        color: MeshColor,
    ) {
        let [cx, cy, cz] = center;
        let [hx, hy, hz] = half;
        let corners = [
            [-hx, -hy, -hz],
            [hx, -hy, -hz],
            [hx, hy, -hz],
            [-hx, hy, -hz],
            [-hx, -hy, hz],
            [hx, -hy, hz],
            [hx, hy, hz],
            [-hx, hy, hz],
        ];
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let vertices = corners.map(|[x, y, z]| {
            // Pitch about Z (tips the +X end), then yaw about Y, then place.
            let (px, py) = (x * cos_pitch - y * sin_pitch, x * sin_pitch + y * cos_pitch);
            [
                cx + px * cos_yaw + z * sin_yaw,
                cy + py,
                cz - px * sin_yaw + z * cos_yaw,
            ]
        });
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

        // The base→shoulder band is darkened: it's the ground-contact zone,
        // and the baked contact shading seats the lump on the terrain
        // instead of letting a bright bottom edge make it float.
        let base_color = scale_rgb(color, 0.72);
        for index in 0..base.len() {
            let next = (index + 1) % base.len();
            self.push_triangle_away_from(
                origin,
                transform(base[index]),
                transform(base[next]),
                transform(shoulder[next]),
                base_color,
            );
            self.push_triangle_away_from(
                origin,
                transform(base[index]),
                transform(shoulder[next]),
                transform(shoulder[index]),
                base_color,
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
        // Underside fan darkened: canopy blobs and embedded chunks read as
        // lit-from-above, with their shadowed belly facing the viewer at
        // eye level.
        let bottom_color = scale_rgb(color, 0.58);
        for index in 0..ring.len() {
            let next = (index + 1) % ring.len();
            self.push_triangle_away_from(center, top, ring[index], ring[next], color);
            self.push_triangle_away_from(center, bottom, ring[next], ring[index], bottom_color);
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

/// Build the shared grass-CARD mesh: `quads` quads crossing at the origin (evenly
/// spaced over a half-turn, e.g. 0/60/120 degrees for 3), each spanning
/// `±half_width` horizontally and `0..height` vertically and UV-mapped to the
/// full grass-tuft texture. The blade detail lives in that texture (mipmapped, so
/// far cards fuse into a soft mass instead of aliasing), not in geometry, so one
/// card replaces a whole tuft of per-blade meshes, the perf + soft-look win.
///
/// Normals point straight up (+Y) so cards light softly like foliage rather than
/// as dark vertical walls (the Ghost-of-Tsushima / Kelemen trick). Vertex-colour
/// rgb is white (colour comes from the texture × per-blade biome tint) and alpha
/// is the height fraction (0 base, 1 top), which doubles as the wind sway weight.
/// Drawn double-sided (the pipeline sets `cull_mode = None`).
pub(crate) fn build_grass_card_mesh(height: f32, half_width: f32, quads: u32) -> Mesh {
    use std::f32::consts::PI;
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for q in 0..quads.max(1) {
        let ang = q as f32 / quads.max(1) as f32 * PI;
        let (s, c) = ang.sin_cos();
        let (wx, wz) = (c * half_width, s * half_width);
        let base = positions.len() as u32;
        // bottom-left, bottom-right, top-right, top-left
        positions.extend_from_slice(&[
            [-wx, 0.0, -wz],
            [wx, 0.0, wz],
            [wx, height, wz],
            [-wx, height, -wz],
        ]);
        normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 4]);
        // v = 1 at the base (texture bottom = blade roots), v = 0 at the top (tips).
        uvs.extend_from_slice(&[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
        // rgb white; alpha = height fraction (0 base, 1 top) = sway weight.
        colors.extend_from_slice(&[
            [1.0, 1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices))
}

/// A harvestable hay-grass tuft as crossed textured quads, the **same grass-tuft
/// texture** as the cosmetic detail grass but bigger and rendered by a plain
/// `StandardMaterial` (warm-tinted, alpha-masked), so it reads as a distinct
/// pickable plant while matching the art style.
///
/// Vertex colour carries a **root→tip brightness gradient in rgb, alpha pinned to
/// 1.0**. `StandardMaterial` multiplies vertex colour into the albedo, so the rgb
/// ramp reproduces the shaded detail grass's root-AO → tip-lift gradient (base
/// darker, tips brighter) that a flat tint otherwise washes out. Alpha stays
/// `1.0` on every vertex on purpose: unlike [`build_grass_card_mesh`] (whose
/// colour alpha is the wind sway weight) the cutout here must come purely from the
/// texture's alpha, so feeding a 0-at-the-base alpha into the mask would chew the
/// bottom of the tuft away. The wind sway is applied per-node on the CPU instead
/// (see `sway_hay_grass_system`), since a `StandardMaterial` can't bend in a shader.
pub(crate) fn build_hay_tuft_mesh(height: f32, half_width: f32, quads: u32) -> Mesh {
    use std::f32::consts::PI;
    // Albedo multiplier at the blade root; tips stay at 1.0. Mimics the detail
    // grass shader's root ambient occlusion so the tuft reads with depth.
    const ROOT_SHADE: f32 = 0.6;
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for q in 0..quads.max(1) {
        let ang = q as f32 / quads.max(1) as f32 * PI;
        let (s, c) = ang.sin_cos();
        let (wx, wz) = (c * half_width, s * half_width);
        let base = positions.len() as u32;
        positions.extend_from_slice(&[
            [-wx, 0.0, -wz],
            [wx, 0.0, wz],
            [wx, height, wz],
            [-wx, height, -wz],
        ]);
        normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 4]);
        // v = 1 at the base (texture bottom = roots), v = 0 at the top (tips).
        uvs.extend_from_slice(&[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
        // rgb root→tip gradient (root darker); alpha = 1 so the cutout is texture-only.
        colors.extend_from_slice(&[
            [ROOT_SHADE, ROOT_SHADE, ROOT_SHADE, 1.0],
            [ROOT_SHADE, ROOT_SHADE, ROOT_SHADE, 1.0],
            [1.0, 1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
        ]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grass_card_mesh_has_quads_with_up_normals_and_sway_alpha() {
        let mesh = build_grass_card_mesh(0.6, 0.2, 3);
        // 3 quads x 4 verts.
        let verts = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(bevy::mesh::VertexAttributeValues::Float32x3(p)) => p.len(),
            _ => 0,
        };
        assert_eq!(verts, 12);
        // Normals all point straight up (+Y) for soft foliage lighting.
        if let Some(bevy::mesh::VertexAttributeValues::Float32x3(normals)) =
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        {
            assert!(normals.iter().all(|n| n[1] > 0.99));
        } else {
            panic!("card mesh has normals");
        }
        // Vertex-colour alpha = height frac: base verts 0, top verts 1 (sway weight).
        if let Some(bevy::mesh::VertexAttributeValues::Float32x4(colors)) =
            mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        {
            assert!(colors.iter().any(|c| c[3] == 0.0));
            assert!(colors.iter().any(|c| c[3] == 1.0));
        } else {
            panic!("card mesh has colours");
        }
    }
}
