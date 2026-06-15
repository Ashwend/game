// Terrain ground material: a standalone PBR material that splat-blends four
// per-biome ground textures (forest / rocky / ore / plains) by a small biome-
// weight raster baked from the same classification noise the world map uses, so
// the floor reads like the map but with real, tiling surface detail.
//
// Standalone `Material` (NOT an `ExtendedMaterial`): it owns the material bind
// group outright, which keeps the texture bindings alive on Metal. An
// ExtendedMaterial extension binding gets dropped in Bevy 0.18's bindless-
// `StandardMaterial` bind-group merge (the reason the grass shader is binding-
// free), so for textures a standalone material is the safe path. The trade-off is
// we rebuild the `PbrInput` by hand instead of calling
// `pbr_input_from_standard_material`; the block below mirrors
// `bevy_pbr::pbr_fragment::pbr_input_from_vertex_output`.
//
// Our material bindings use `@group(#{MATERIAL_BIND_GROUP})`, NOT a literal group
// number: in Bevy 0.18 the per-object mesh array sits at the literal `@group(2)`
// (`mesh_bindings`), and the material bind group is group 3 via that shaderdef.
// Hardcoding `@group(2)` collides with the mesh binding. We also deliberately do
// NOT import `pbr_fragment`: it pulls in `pbr_bindings`, a second
// `StandardMaterial` binding set in the same material group.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types,
    pbr_types::{PbrInput, pbr_input_new},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing, calculate_view, prepare_world_normal},
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}

// The terrain material's own bindings (the material bind group).
//   params.x = floor_size  (m): the origin-centred ground plane's side length.
//   params.y = tile_size   (m): world metres per repeat of each biome texture.
//   params.z = fade_start   (m): camera distance where detail starts fading to flat.
//   params.w = fade_end     (m): camera distance where detail is fully flat.
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: vec4<f32>;
// Per-world biome weights, RGBA = forest, rocky, ore, plains; clamp + linear.
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var weights_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var weights_samp: sampler;
// The four shared per-biome ground textures; repeat + linear (`albedo_samp`).
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var forest_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var rocky_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var ore_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var plains_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var albedo_samp: sampler;

// Flat per-biome palette in LINEAR space, matching the world-map colours (the map
// uses the same sRGB bytes: forest #3c6c38, rocky #7e7c76, ore #7a6048, plains
// #96ac60). The distance fade resolves the tiled detail toward this, so far
// terrain reads as the flat map and its tiling/minification artefacts vanish.
const PAL_FOREST = vec3<f32>(0.04516, 0.14989, 0.03956);
const PAL_ROCKY = vec3<f32>(0.20856, 0.20156, 0.18117);
const PAL_ORE = vec3<f32>(0.19462, 0.11696, 0.06479);
const PAL_PLAINS = vec3<f32>(0.30505, 0.41269, 0.11696);

// Anti-tiling tuning. A small, slowly-varying domain-warp OFFSET of the detail UV
// breaks the global 7 m grid alignment; a broad low-frequency brightness wash
// hides the residual repeat perceptually. Both are nearly free, no extra texture
// taps. NB: we offset, never rotate the global UV, a position-varying rotation of
// `world_xz` amplifies with distance from the origin and smears the texture; a
// bounded offset keeps UV derivatives sane everywhere.
const WARP_SCALE_M: f32 = 55.0; // metres per warp period (large = gentle, low-freq warp)
const WARP_TILES: f32 = 2.0; // peak UV displacement, in tile widths
const MACRO_SCALE_M: f32 = 38.0; // metres per macro brightness patch
const MACRO_MIN: f32 = 0.84; // patch tint range; mostly darken-only, like the grass shader
const MACRO_MAX: f32 = 1.10;

// --- Binding-free value noise (Dave Hoskins hash, no `sin` banding), copied from
// the grass shader so the terrain shares the same stylised macro-variation. ---
fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.xyx) * 0.1031);
    p3 = p3 + dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash12(i);
    let b = hash12(i + vec2<f32>(1.0, 0.0));
    let c = hash12(i + vec2<f32>(0.0, 1.0));
    let d = hash12(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm2(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var amp = 0.5;
    var freq = 1.0;
    for (var o = 0; o < 3; o = o + 1) {
        v = v + amp * value_noise(p * freq);
        freq = freq * 2.0;
        amp = amp * 0.5;
    }
    return v;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    let floor_size = params.x;
    let tile_size = params.y;
    let world_xz = in.world_position.xz;

    // Biome weights. The plane is centred on the origin with side `floor_size`,
    // so world XZ maps straight into the [0,1] weight texture. Renormalise after
    // the bilinear tap so the blend stays convex (the bake stores near-one-hot
    // biome interiors, but filtering can leave the sum a hair off 1).
    let wuv = world_xz / floor_size + vec2<f32>(0.5, 0.5);
    var w = textureSample(weights_tex, weights_samp, wuv);
    let wsum = w.r + w.g + w.b + w.a;
    if wsum > 0.0001 {
        w = w / wsum;
    } else {
        w = vec4<f32>(0.25, 0.25, 0.25, 0.25);
    }

    // Per-biome surface detail, sampled in world space so the grain is continuous
    // across the whole floor (neighbouring biomes share it, no per-tile seams).
    // Anti-tiling: domain-warp the detail UV by a small, slowly-varying offset so
    // the 7 m grid never lines up into a visible repeat. The warp is a bounded
    // OFFSET (not a rotation of the global coordinate, which would amplify with
    // distance and smear), so UV derivatives stay sane everywhere and there are no
    // seams (the fbm field is continuous).
    let warp = vec2<f32>(
        fbm2(world_xz / WARP_SCALE_M),
        fbm2(world_xz / WARP_SCALE_M + vec2<f32>(37.0, 19.0)),
    ) - vec2<f32>(0.5, 0.5);
    let tuv = world_xz / tile_size + warp * WARP_TILES;
    let c_forest = textureSample(forest_tex, albedo_samp, tuv).rgb;
    let c_rocky = textureSample(rocky_tex, albedo_samp, tuv).rgb;
    let c_ore = textureSample(ore_tex, albedo_samp, tuv).rgb;
    let c_plains = textureSample(plains_tex, albedo_samp, tuv).rgb;
    var albedo = c_forest * w.r + c_rocky * w.g + c_ore * w.b + c_plains * w.a;

    // Macro variation: a broad low-frequency brightness wash (with a faint
    // warm/cool hue break) over the tiled grain, so the eye stops reading the
    // repeat. Cheap: one fbm eval, no extra taps. Mirrors the grass shader's
    // patch tint and stays under the daylight exposure so no biome blooms.
    let macro_n = fbm2(world_xz / MACRO_SCALE_M);
    let macro_t = mix(MACRO_MIN, MACRO_MAX, macro_n);
    albedo = albedo * vec3<f32>(macro_t * 1.01, macro_t, macro_t * 0.98);

    // Distance detail-fade: resolve the tiled detail toward the flat biome map
    // colour as the camera distance grows. This kills the far-field minification
    // shimmer that mips/anisotropy can't fully save (sub-pixel tiles) AND hides
    // the far tiling, landing on the flat map look, which is exactly on-style.
    let flat_albedo = PAL_FOREST * w.r + PAL_ROCKY * w.g + PAL_ORE * w.b + PAL_PLAINS * w.a;
    let cam_dist = distance(world_xz, view.world_position.xz);
    let detail_fade = smoothstep(params.z, params.w, cam_dist);
    let final_albedo = mix(albedo, flat_albedo, detail_fade);

    // Rebuild the PbrInput by hand (see header). Mirrors the vertex-output half of
    // `pbr_input_from_vertex_output`: the mesh flags carry the shadow-receiver
    // bit, so the floor still takes tree/building shadows.
    var pbr_input: PbrInput = pbr_input_new();
    pbr_input.flags = mesh[in.instance_index].flags;
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.V = calculate_view(in.world_position, pbr_input.is_orthographic);
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = prepare_world_normal(in.world_normal, false, is_front);
    pbr_input.N = normalize(pbr_input.world_normal);

    // Matte ground: full roughness, zero reflectance, so the flat floor never
    // shows the Fresnel "wet glass" specular band under a low sun. Lit by the
    // same sun + atmosphere IBL as everything else.
    pbr_input.material.base_color = vec4<f32>(final_albedo, 1.0);
    pbr_input.material.perceptual_roughness = 1.0;
    pbr_input.material.reflectance = vec3<f32>(0.0, 0.0, 0.0);
    pbr_input.material.metallic = 0.0;
    // Fade into the same distance-fog haze as the rest of the scene.
    pbr_input.material.flags = pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
