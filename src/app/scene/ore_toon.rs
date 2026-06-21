//! Cel-shaded ore/vein material. A standalone Bevy [`Material`] (same shape as
//! [`super::terrain::TerrainMaterial`]) that toon-shades the ore-node boulders so
//! the hand-painted rock texture reads anime/cartoon instead of being smoothly
//! PBR-lit. One shared material covers all four ores: the per-mineral colour
//! rides on the glb `COLOR_0` (grey rock body vs bright mineral chunks) and the
//! rock detail texture is shared, so `texture * COLOR_0` differentiates them and
//! every ore node batches by one material. Shader: `assets/shaders/ore_toon.wgsl`.
//! See [Materials](../../../docs/materials.md) for the standalone-Material /
//! Metal bind-group reasoning shared with the terrain material.

use bevy::{
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

/// Embedded path of the ore toon shader (a `&'static str` because [`ShaderRef`]
/// needs one; same `embedded://` scheme as the terrain material).
const ORE_TOON_SHADER_PATH: &str = "embedded://shaders/ore_toon.wgsl";

/// Standalone cel-shaded material for ore/vein nodes. Bindings map 1:1 with
/// `assets/shaders/ore_toon.wgsl`.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct OreToonMaterial {
    /// Shared grey-rock detail texture (`textures/ore/rock.png`); the binding-1
    /// sampler is repeat + linear, taken from this image's sampler. The shader
    /// multiplies it by the mesh `COLOR_0` so the per-mineral chunks read.
    #[texture(0)]
    #[sampler(1)]
    pub(crate) rock: Handle<Image>,
    /// Cel tuning, packed so it can be tweaked without a recompile.
    /// `x = band count`, `y = ambient floor`, `z = rim strength`, `w = unused`.
    #[uniform(2)]
    pub(crate) params: Vec4,
}

impl Material for OreToonMaterial {
    fn fragment_shader() -> ShaderRef {
        ORE_TOON_SHADER_PATH.into()
    }
}
