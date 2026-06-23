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
//! extend it, and [Rendering materials](../../../docs/rendering-materials.md) for the
//! standalone-Material / Metal bind-group reasoning shared with the terrain
//! material.

use bevy::{prelude::*, render::render_resource::AsBindGroup, shader::ShaderRef};

/// Embedded path of the toon shader (a `&'static str` because [`ShaderRef`]
/// needs one; same `embedded://` scheme as the terrain material).
const TOON_SHADER_PATH: &str = "embedded://shaders/toon.wgsl";

/// Embedded path of the toon **prepass** fragment shader. Required so the
/// alpha-masked grass-card tufts discard their transparent texels in the
/// depth/motion prepass that TAA adds; without it the stock prepass writes the
/// full opaque quad and the cards render as black holes under TAA. See the
/// shader's header for the full explanation.
const TOON_PREPASS_SHADER_PATH: &str = "embedded://shaders/toon_prepass.wgsl";

/// Embedded path of the first-person held-tool ("viewmodel") cel shader. Same
/// bind group as [`ToonMaterial`] but a camera-relative key light so the cel bands
/// stay stable as the camera turns instead of swimming with the world sun.
const TOON_VIEWMODEL_SHADER_PATH: &str = "embedded://shaders/toon_viewmodel.wgsl";

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
    /// `x = cel band count`, `y = alpha-mask cutoff` (0 = opaque; >0 turns the
    /// material into an alpha-masked, double-sided grass-card tuft, discarding
    /// texture alpha below the cutoff), `z = ink-edge strength`,
    /// `w = ink-edge width exponent`.
    #[uniform(2)]
    pub(crate) params: Vec4,
    /// Texture tiles per metre for the **triplanar** path used by meshes without
    /// UVs (the deployable props). Ignored by meshes that carry their own UVs
    /// (the ore glbs), which sample `detail` directly.
    #[uniform(3)]
    pub(crate) tex_scale: f32,
    /// Per-instance opacity, `1.0` for every static prop. Only the tree-felling
    /// dissolve drives it below `1.0` (on a *cloned* material) so the banded
    /// trunk + canopy fade smoothly to nothing as the felled tree despawns; the
    /// shader multiplies the output alpha by it. Sub-1.0 also flips
    /// [`Self::alpha_mode`] to [`AlphaMode::Blend`] so the fade actually blends.
    #[uniform(4)]
    pub(crate) fade: f32,
}

impl Material for ToonMaterial {
    fn fragment_shader() -> ShaderRef {
        TOON_SHADER_PATH.into()
    }

    /// Custom prepass fragment so the alpha-masked grass cards discard their
    /// transparent texels in the depth/motion prepass (added by TAA), matching
    /// the main pass. Opaque toon props (params.y == 0) pass through unchanged.
    fn prepass_fragment_shader() -> ShaderRef {
        TOON_PREPASS_SHADER_PATH.into()
    }

    /// Opaque in normal use (`fade == 1.0`), so cel props draw in the cheap
    /// opaque pass and depth-occlude the transparent detail grass correctly.
    /// The felling dissolve lowers `fade` on its private clone, flipping that
    /// one material into the transparent pass for the fade-out (mirrors what the
    /// old `StandardMaterial` trunk did when it set `AlphaMode::Blend`).
    ///
    /// Grass-card tufts (`params.y > 0`) instead use [`AlphaMode::Mask`] with the
    /// cutoff in `params.y`: the silhouette is a hard cut-out, so the card draws
    /// in the opaque/alpha-mask pass (depth-correct, no sort) while the shader
    /// discards the transparent gaps.
    fn alpha_mode(&self) -> AlphaMode {
        if self.params.y > 0.0 {
            AlphaMode::Mask(self.params.y)
        } else if self.fade < 1.0 {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        }
    }
}

/// Cel material for the FIRST-PERSON held tool (a camera-child viewmodel). Same
/// bind group + fields as [`ToonMaterial`], but its shader lights the cel bands
/// with a key light fixed in *view* space, so the bands stay put as the camera
/// turns instead of swimming with the world sun (the standard "viewmodel light
/// rig" trick). Day/night brightness still tracks the scene via an
/// orientation-independent probe in the shader. Only the in-hand item uses this;
/// the third-person tool on remote players stays on [`ToonMaterial`]. Shader:
/// `assets/shaders/toon_viewmodel.wgsl`.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct ToonViewmodelMaterial {
    /// Surface detail texture; the shader does `detail * COLOR_0` like the world
    /// toon material. The binding-1 sampler is taken from this image.
    #[texture(0)]
    #[sampler(1)]
    pub(crate) detail: Handle<Image>,
    /// Cel tuning: `x = band count`, `y = alpha-mask cutoff` (0 for the opaque
    /// tools), `z = ink-edge strength`, `w = ink-edge width exponent`.
    #[uniform(2)]
    pub(crate) params: Vec4,
    /// Triplanar tiles/metre; unused by the UV'd tool glbs (kept so the bind group
    /// matches `ToonMaterial`'s layout).
    #[uniform(3)]
    pub(crate) tex_scale: f32,
    /// Per-instance opacity; `1.0` for the tools (no felling dissolve here).
    #[uniform(4)]
    pub(crate) fade: f32,
}

impl Material for ToonViewmodelMaterial {
    fn fragment_shader() -> ShaderRef {
        TOON_VIEWMODEL_SHADER_PATH.into()
    }

    /// The held tools are always opaque (`fade == 1.0`, `params.y == 0`), so the
    /// default opaque prepass is fine and no custom prepass shader is needed.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }
}
