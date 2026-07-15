//! Loose material/texture-building helpers shared by the `setup_scene`
//! asset-builder functions in `assets.rs`.

use bevy::{
    image::{ImageAddressMode, ImageFilterMode, ImageSamplerDescriptor},
    prelude::*,
};

use super::toon::ToonMaterial;

/// Build one toony tall-grass (hay) material from a seed-headed tuft card. It is
/// the shared cel-shaded [`ToonMaterial`], so the harvestable plant is lit by the
/// real PBR sun + atmosphere IBL + day/night exposure exactly like the cosmetic
/// detail grass, the trees, and the ore nodes (the old plain `StandardMaterial`
/// sat outside that PBR-then-posterise path and read flat against the cel world).
/// `params`: `x = 3` cel bands (matches the detail grass), `y = 0.4` is the alpha
/// cutoff that turns this into an alpha-masked, double-sided card (the blade
/// silhouette lives in the texture's alpha), `z = 0` disables the ink-edge outline
/// (grass blades want no drawn silhouette). The painted texture supplies the green;
/// the mesh COLOR_0 root→tip ramp tints it (`detail * COLOR_0`).
pub(super) fn hay_tall_grass_material(
    tex: Handle<Image>,
    no_glow_tex: Handle<Image>,
) -> ToonMaterial {
    ToonMaterial {
        detail: tex,
        params: Vec4::new(3.0, 0.4, 0.0, 2.0),
        tex_scale: 1.0, // the hay card carries its own UVs; triplanar scale unused
        fade: 1.0,
        dev_flags: 0,
        emissive_tex: no_glow_tex,
        emissive: Vec4::ZERO,
    }
}

/// Repeat + anisotropic trilinear sampler for the tree bark/canopy textures, so
/// bark tiles up the trunk and the needle/leaf texture tiles across the canopy
/// shells without a visible seam, and stays crisp (not shimmery) at distance.
/// Mirrors the terrain ground sampler; only meaningful with a mip chain
/// (`build_mip_chain`), which the tree-texture loader builds.
pub(super) fn tree_texture_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    }
}
