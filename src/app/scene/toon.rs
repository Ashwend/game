//! Shared cel-shaded (toon / anime) material. A standalone Bevy [`Material`]
//! (same shape as [`super::terrain::TerrainMaterial`]) that **PBR-then-posterises**
//! a prop: the shader lights it with the real engine lighting (`apply_pbr_lighting`,
//! so it inherits the scene's day/night exposure + received shadows + IBL like the
//! ground) and then quantises the lit luminance into a few hard cel bands for the
//! anime look. Used by the ore-node boulders and the deployable props (workbench,
//! furnace, storage, torch, tool cupboard, sleeping bag). Lighting day/night is
//! therefore handled by the engine, no day-factor uniform needed.
//!
//! The surface colour is `detail_texture * COLOR_0`: the per-prop colour rides on
//! the glb `COLOR_0` vertex colours and the `detail` texture adds surface grain.
//! Props that have no texture (the vertex-colour-only deployables) bind a 1x1
//! white `detail`, so the multiply reduces to pure `COLOR_0`, the same result the
//! old base-white `StandardMaterial` gave. Shader: `assets/shaders/toon.wgsl`.
//! See [Toon / cel shading](../../../docs/toon-shading.md) for the style + how to
//! extend it, and [Materials](../../../docs/materials.md) for the
//! standalone-Material / Metal bind-group reasoning shared with the terrain
//! material.

use bevy::{prelude::*, render::render_resource::AsBindGroup, shader::ShaderRef};

/// Embedded path of the toon shader (a `&'static str` because [`ShaderRef`]
/// needs one; same `embedded://` scheme as the terrain material).
const TOON_SHADER_PATH: &str = "embedded://shaders/toon.wgsl";

/// Standalone cel-shaded material. Bindings map 1:1 with
/// `assets/shaders/toon.wgsl`.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct ToonMaterial {
    /// Surface detail texture (e.g. the ore rock grain, a building tier texture);
    /// bind a 1x1 white image for vertex-colour-only props. The binding-1 sampler
    /// is taken from this image's sampler. The shader does `detail * COLOR_0`.
    #[texture(0)]
    #[sampler(1)]
    pub(crate) detail: Handle<Image>,
    /// Cel tuning, packed so it can be tweaked without a recompile.
    /// `x = cel band count`, `y = unused` (was the flat ambient floor; PBR now
    /// supplies ambient via IBL), `z = ink-edge strength`,
    /// `w = ink-edge width exponent`.
    #[uniform(2)]
    pub(crate) params: Vec4,
    /// Texture tiles per metre for the **triplanar** path used by meshes without
    /// UVs (the deployable props). Ignored by meshes that carry their own UVs
    /// (the ore glbs), which sample `detail` directly.
    #[uniform(3)]
    pub(crate) tex_scale: f32,
}

impl Material for ToonMaterial {
    fn fragment_shader() -> ShaderRef {
        TOON_SHADER_PATH.into()
    }
}
