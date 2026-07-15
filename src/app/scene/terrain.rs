//! Textured ground floor: a standalone PBR [`TerrainMaterial`] that splat-blends
//! four per-biome ground textures by a per-world biome-weight raster, so the
//! floor reads like the world map but with real, tiling surface detail.
//!
//! The four biome textures are shared across all worlds and loaded once into
//! [`TerrainTextureAssets`]; only the small biome-weight image differs per world
//! (baked on the CPU from the seed by [`crate::world::render_terrain_weight_rgba`],
//! the same noise the map uses). Shader: `assets/shaders/terrain.wgsl`. See
//! [Rendering materials](../../../docs/rendering-materials.md) for the PBR conventions and the
//! standalone-vs-ExtendedMaterial reasoning.
//!
//! Heightmap note: displacement is intentionally NOT done yet. When it lands it
//! attaches to the already-subdivided ground mesh (`super::world::build_ground_mesh`)
//! and would feed slope into the blend here; nothing in this module needs a
//! redesign for it.

use bevy::{
    asset::RenderAssetUsages,
    image::{
        CompressedImageFormats, ImageAddressMode, ImageFilterMode, ImageSampler,
        ImageSamplerDescriptor, ImageType,
    },
    prelude::*,
    render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat},
    shader::ShaderRef,
    tasks::ComputeTaskPool,
};

use crate::{
    app::embedded_assets::embedded_bytes,
    world::{TERRAIN_WEIGHT_TEXELS, fill_terrain_weight_rows},
};

/// Embedded path of the terrain shader (a `&'static str` because [`ShaderRef`]
/// needs one; same `embedded://` scheme as [`crate::app::embedded_asset_path`]).
const TERRAIN_SHADER_PATH: &str = "embedded://shaders/terrain.wgsl";

/// World metres per repeat of each per-biome ground texture. Small enough that
/// the surface reads as ground underfoot, large enough that the tile repeat is
/// not obvious at a glance; the biome-weight blend varies the surface across the
/// map on top of this.
const TERRAIN_TILE_SIZE_M: f32 = 7.0;

/// Camera distance (m) where the tiled ground detail starts fading toward the
/// flat per-biome map colour, and where it's fully faded. The shader does the
/// fade ([`assets/shaders/terrain.wgsl`]); it both hides the far tile repeat and
/// resolves the residual minification shimmer into the flat map palette (which is
/// the look we want at range). Tuned to land inside the daytime fog band so the
/// hand-off isn't conspicuous. Carried in `params.z`/`params.w`.
const TERRAIN_FADE_START_M: f32 = 55.0;
const TERRAIN_FADE_END_M: f32 = 200.0;

/// The four shared per-biome ground textures (loaded once, repeat-sampled),
/// reused by every world's [`TerrainMaterial`]. Only the per-world biome-weight
/// raster differs between worlds.
#[derive(Resource, Clone)]
pub(crate) struct TerrainTextureAssets {
    forest: Handle<Image>,
    rocky: Handle<Image>,
    ore: Handle<Image>,
    plains: Handle<Image>,
}

impl TerrainTextureAssets {
    /// Decode the embedded biome PNGs, build a mip chain for each, and add them to
    /// `Assets<Image>` with a repeat + anisotropic sampler. We decode the embedded
    /// bytes synchronously (rather than the async `asset_server.load` path) so we
    /// can build mips up front: Bevy 0.18 does not generate mipmaps for loaded
    /// PNGs, and without them the 7 m-tiled ground aliases badly into the distance.
    pub(crate) fn load(images: &mut Assets<Image>) -> Self {
        let mut load = |name: &str| -> Handle<Image> {
            let rel = format!("textures/terrain/{name}.png");
            let bytes = embedded_bytes(&rel)
                .unwrap_or_else(|| panic!("embedded terrain texture missing: {rel}"));
            let mut image = Image::from_buffer(
                bytes,
                ImageType::Extension("png"),
                CompressedImageFormats::NONE,
                // Albedo: sRGB, so the sampler hands the shader linear colour.
                true,
                ImageSampler::Descriptor(repeat_linear_sampler()),
                // GPU-only: we keep no CPU copy once the mip chain is uploaded.
                RenderAssetUsages::RENDER_WORLD,
            )
            .unwrap_or_else(|err| panic!("decode terrain texture {rel}: {err:?}"));
            build_mip_chain(&mut image);
            images.add(image)
        };
        Self {
            forest: load("forest"),
            rocky: load("rocky"),
            ore: load("ore"),
            plains: load("plains"),
        }
    }
}

/// Standalone splat-blend ground material. Owns `@group(2)` outright so its
/// texture bindings survive Metal (see the module/shader headers). Bindings line
/// up 1:1 with `assets/shaders/terrain.wgsl`.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct TerrainMaterial {
    /// `x = floor_size` (m), `y = tile_size` (m), `z = detail-fade start` (m),
    /// `w = detail-fade end` (m).
    #[uniform(0)]
    params: Vec4,
    /// Per-world biome weights (R forest, G rocky, B ore, A plains); the binding-2
    /// sampler is clamp + linear, taken from this image's sampler.
    #[texture(1)]
    #[sampler(2)]
    weights: Handle<Image>,
    #[texture(3)]
    forest: Handle<Image>,
    #[texture(4)]
    rocky: Handle<Image>,
    #[texture(5)]
    ore: Handle<Image>,
    /// The binding-7 sampler is repeat + linear (from this image's sampler) and is
    /// shared by all four biome textures in the shader.
    #[texture(6)]
    #[sampler(7)]
    plains: Handle<Image>,
}

impl Material for TerrainMaterial {
    fn fragment_shader() -> ShaderRef {
        TERRAIN_SHADER_PATH.into()
    }
}

/// Build a [`TerrainMaterial`] for a world: bake its biome-weight raster from the
/// seed and assemble it with the shared biome textures.
pub(crate) fn build_terrain_material(
    world_seed: u64,
    floor_size: f32,
    textures: &TerrainTextureAssets,
    images: &mut Assets<Image>,
    materials: &mut Assets<TerrainMaterial>,
) -> Handle<TerrainMaterial> {
    let weights = images.add(terrain_weight_image(world_seed, floor_size));
    materials.add(TerrainMaterial {
        params: Vec4::new(
            floor_size,
            TERRAIN_TILE_SIZE_M,
            TERRAIN_FADE_START_M,
            TERRAIN_FADE_END_M,
        ),
        weights,
        forest: textures.forest.clone(),
        rocky: textures.rocky.clone(),
        ore: textures.ore.clone(),
        plains: textures.plains.clone(),
    })
}

/// The CPU-baked biome-weight raster as a clamp-sampled **linear** image
/// (`Rgba8Unorm`, not sRGB, the channels are weights, not colour).
fn terrain_weight_image(world_seed: u64, floor_size: f32) -> Image {
    let rgba = bake_terrain_weight_parallel(world_seed, floor_size);
    let mut image = Image::new(
        Extent3d {
            width: TERRAIN_WEIGHT_TEXELS,
            height: TERRAIN_WEIGHT_TEXELS,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(clamp_linear_sampler());
    image
}

/// Bake the `TERRAIN_WEIGHT_TEXELS²` biome-weight raster across the compute task
/// pool instead of on the single calling frame.
///
/// The raster is a pure function of `(world_seed, floor_size)` and its rows are
/// independent, so we split it into horizontal bands and fill each into a
/// disjoint slice in parallel. The byte output is identical to a serial
/// [`render_terrain_weight_rgba`](crate::world::render_terrain_weight_rgba) bake
/// (locked by `banded_fill_matches_serial_render`). The whole bake is ~2.1M
/// value-noise evaluations that runs once on world load; serial it was a ~250ms
/// stall on the world-entry frame, fanning it out cuts that by roughly the core
/// count. This is pure client-side rendering derived from the world seed, so it
/// is identical for singleplayer loopback and remote multiplayer sessions.
fn bake_terrain_weight_parallel(world_seed: u64, floor_size: f32) -> Vec<u8> {
    let texels = TERRAIN_WEIGHT_TEXELS;
    let mut rgba = vec![0u8; (texels * texels * 4) as usize];
    let row_bytes = (texels * 4) as usize;

    // The pool is initialised by Bevy's `TaskPoolPlugin`; `get_or_init` keeps
    // the bake working in any headless/test context that calls this directly.
    let pool = ComputeTaskPool::get_or_init(Default::default);
    let threads = pool.thread_num().max(1);
    // A few bands per worker so uneven per-row noise cost still load-balances.
    let bands = (threads * 4).clamp(1, texels as usize);
    let rows_per_band = texels.div_ceil(bands as u32);

    pool.scope(|scope| {
        let mut remaining = rgba.as_mut_slice();
        let mut row_start = 0u32;
        while row_start < texels {
            let row_end = (row_start + rows_per_band).min(texels);
            let band_len = (row_end - row_start) as usize * row_bytes;
            let (band, rest) = remaining.split_at_mut(band_len);
            remaining = rest;
            scope.spawn(async move {
                fill_terrain_weight_rows(world_seed, floor_size, row_start, row_end, band);
            });
            row_start = row_end;
        }
    });

    rgba
}

fn repeat_linear_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        // Trilinear + anisotropic: only meaningful once the textures carry a mip
        // chain (see `build_mip_chain`). Anisotropy is what keeps the ground crisp
        // at grazing angles, the worst minification case for a near-flat floor.
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    }
}

/// Box-downsample a full mip chain into `image.data` and set `mip_level_count`,
/// so the GPU can trilinear/anisotropically filter the tiled ground at distance
/// instead of aliasing it into shimmering noise.
///
/// Bevy 0.18 doesn't generate mips for loaded PNGs and ships no runtime mip util,
/// so we build the chain on the CPU once at startup (~1 ms per 768px texture). The
/// format is `Rgba8UnormSrgb`, so colour channels are averaged in **linear** space
/// (decode -> average -> re-encode); alpha is linear already. Levels are appended
/// in order, which is exactly what wgpu's `create_texture_with_data` upload reads
/// for a single-layer 2D image (default `LayerMajor` == mip-major here).
pub(crate) fn build_mip_chain(image: &mut Image) {
    let mut w = image.width();
    let mut h = image.height();
    let mut src = image.data.clone().expect("decoded image has pixel data");
    let mut levels = 1u32;

    // The per-texel sRGB<->linear conversions are cheap in a release build (~1 ms
    // per 768px texture) but slow at the dev opt-level 0: serial, the ~20 textures
    // decoded at startup cost ~1.4 s of mip building and froze the first frame (the
    // spinner can't animate while the main thread is blocked). A LUT doesn't help
    // because opt-0 makes the array indexing just as slow; the dst rows of each
    // level are independent, so fan them out across the compute pool (same pattern
    // as `bake_terrain_weight_parallel`), which cuts the dev cost by ~the core count.
    let pool = ComputeTaskPool::get_or_init(Default::default);
    let threads = pool.thread_num().max(1);

    while w > 1 || h > 1 {
        let nw = (w / 2).max(1);
        let nh = (h / 2).max(1);
        let mut dst = vec![0u8; (nw * nh * 4) as usize];
        let row_bytes = (nw * 4) as usize;

        if (nh as usize) <= threads {
            // Too few rows to be worth the fan-out overhead; downsample serially.
            fill_mip_rows(&src, w, h, nw, 0, nh, &mut dst);
        } else {
            let bands = (threads * 4).clamp(1, nh as usize);
            let rows_per_band = nh.div_ceil(bands as u32);
            pool.scope(|scope| {
                let src_ref = &src;
                let mut remaining = dst.as_mut_slice();
                let mut row_start = 0u32;
                while row_start < nh {
                    let row_end = (row_start + rows_per_band).min(nh);
                    let band_len = (row_end - row_start) as usize * row_bytes;
                    let (band, rest) = remaining.split_at_mut(band_len);
                    remaining = rest;
                    scope.spawn(async move {
                        fill_mip_rows(src_ref, w, h, nw, row_start, row_end, band);
                    });
                    row_start = row_end;
                }
            });
        }

        image
            .data
            .as_mut()
            .expect("pixel data present")
            .extend_from_slice(&dst);
        src = dst;
        w = nw;
        h = nh;
        levels += 1;
    }

    image.texture_descriptor.mip_level_count = levels;
}

/// Box-downsample source rows `[row_start, row_end)` of the `w`x`sh` source into
/// `dst_band`, which holds exactly those destination rows (each `nw` texels wide).
/// Colours are averaged in linear space; alpha is already linear. Split out so the
/// bands can be filled in parallel (see [`build_mip_chain`]).
fn fill_mip_rows(
    src: &[u8],
    w: u32,
    sh: u32,
    nw: u32,
    row_start: u32,
    row_end: u32,
    dst_band: &mut [u8],
) {
    for y in row_start..row_end {
        for x in 0..nw {
            let mut acc = [0.0f32; 4];
            for dy in 0..2 {
                for dx in 0..2 {
                    let sx = (x * 2 + dx).min(w - 1);
                    let sy = (y * 2 + dy).min(sh - 1);
                    let i = ((sy * w + sx) * 4) as usize;
                    acc[0] += srgb_to_linear(src[i]);
                    acc[1] += srgb_to_linear(src[i + 1]);
                    acc[2] += srgb_to_linear(src[i + 2]);
                    acc[3] += src[i + 3] as f32 / 255.0;
                }
            }
            let o = ((y - row_start) * nw + x) as usize * 4;
            dst_band[o] = linear_to_srgb(acc[0] * 0.25);
            dst_band[o + 1] = linear_to_srgb(acc[1] * 0.25);
            dst_band[o + 2] = linear_to_srgb(acc[2] * 0.25);
            dst_band[o + 3] = (acc[3] * 0.25 * 255.0).round() as u8;
        }
    }
}

fn srgb_to_linear(c: u8) -> f32 {
    let x = c as f32 / 255.0;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(x: f32) -> u8 {
    let y = if x <= 0.0031308 {
        x * 12.92
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    (y.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn clamp_linear_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_chain_has_expected_levels_and_tight_byte_layout() {
        // 4x4 -> mip dims 4,2,1 = 3 levels; wgpu expects each level tightly packed
        // and concatenated, so total bytes = (16 + 4 + 1) * 4. A mismatch here is
        // exactly what panics `create_texture_with_data` at upload.
        let mut image = Image::new(
            Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![128u8; 4 * 4 * 4],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        build_mip_chain(&mut image);
        assert_eq!(image.texture_descriptor.mip_level_count, 3);
        assert_eq!(image.data.as_ref().unwrap().len(), (16 + 4 + 1) * 4);
    }

    #[test]
    fn non_power_of_two_mip_levels_match_wgpu_flooring() {
        // 768 floors to 768,384,...,3,1 = 10 levels (1 + floor(log2(768))).
        let mut image = Image::new(
            Extent3d {
                width: 768,
                height: 768,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![0u8; 768 * 768 * 4],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        build_mip_chain(&mut image);
        assert_eq!(image.texture_descriptor.mip_level_count, 10);
    }

    #[test]
    fn srgb_conversion_round_trips_endpoints() {
        assert_eq!(linear_to_srgb(srgb_to_linear(0)), 0);
        assert_eq!(linear_to_srgb(srgb_to_linear(255)), 255);
    }
}
