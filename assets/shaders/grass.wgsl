// Detail-grass material: an `ExtendedMaterial<StandardMaterial, GrassWindExtension>`.
//
// It keeps StandardMaterial's PBR lighting (so grass is lit by the same sun +
// atmosphere IBL as the rest of the scene) and layers on three effects:
//
//   * Vertex wind sway, tips displace along a world-space travelling wave,
//     weighted by the per-vertex sway weight baked into vertex-colour alpha
//     (0 at the blade base, 1 at the tip), so blades bend rather than slide.
//   * Fragment radial dither, whole blades are discarded with increasing
//     probability as their distance from the camera grows, thinning the field
//     into smooth rings (no hard cutoff line, no square tile boundaries). The
//     discard key is a stable per-blade random stored in `uv.x`, so a blade is
//     all-or-nothing every frame (no holes, no shimmer).
//   * Stylized world-space colour patches, a low-frequency procedural noise in
//     world XZ tints the albedo lighter/darker across the field, the hand-painted
//     "patchy lawn" look from the StayAtHomeDev Godot stylized-grass shader
//     (`ALBEDO *= mix(color, color2, noise)`). It's procedural rather than a
//     sampled texture so the material stays binding-free (see below).

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{view, globals},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
}

// All tuning is compile-time constants. Deliberately NO material (group-2)
// uniform: `ExtendedMaterial`'s bind-group merge with the bindless
// `StandardMaterial` on Metal drops a `@binding(100)` extension uniform from the
// pipeline layout, so referencing one here crashes at pipeline creation
// ("binding 100 missing from pipeline layout"). Keeping the shader free of any
// extension binding sidesteps that entirely, the trade-off is the fade window
// is fixed (so the draw radius is the same across density tiers; density only
// changes blade count).

// Radial-dither fade window (metres from camera): full density before `start`,
// fully thinned by `end`, just inside the tile despawn radius so grass is gone
// before a tile despawns (no pop).
const FADE_START: f32 = 18.0;
const FADE_END: f32 = 45.0;

// Wind tuning. Strength = horizontal tip sway in metres; speed scales the
// temporal wave; scale = spatial frequency (radians/metre, lower = longer,
// rolling gusts).
const WIND_STRENGTH: f32 = 0.04;
const WIND_SPEED: f32 = 1.15;
const WIND_SCALE: f32 = 0.09;

// Stylized colour-patch tuning. `NOISE_SCALE` is metres per noise cell (Godot's
// `noiseScale`, larger = broader patches). The albedo is multiplied by a tint
// that runs from `PATCH_MIN` (darker patches) to `PATCH_MAX` (lighter), with a
// faint warm/cool hue shift so bright patches read a touch yellower, mimicking
// the two-green `mix(color, color2, noise)` of the reference shader. Kept mostly
// darken-only (range straddles 1.0 only slightly) so patches never glow past the
// ground tone under the scene's daylight-calibrated exposure.
const NOISE_SCALE: f32 = 14.0;
const PATCH_MIN: f32 = 0.72;
const PATCH_MAX: f32 = 1.10;

// --- Procedural value noise (binding-free; replaces the reference's sampled
// noise texture). Dave Hoskins' hash (no `sin`, so no precision banding at large
// world coords) + bilinear value noise + 3-octave fBm. ---
fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.xyx) * 0.1031);
    p3 = p3 + dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    // Smoothstep weights for C1-continuous interpolation (no grid creases).
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

// World-space horizontal wind displacement at a point, before the per-vertex
// sway weight. A primary wave plus a faster, smaller secondary wave keeps it
// from looking like a single clean sine.
fn wind_offset(world_xz: vec2<f32>, t: f32) -> vec2<f32> {
    let phase = world_xz.x * WIND_SCALE + world_xz.y * (WIND_SCALE * 0.7);
    let wave = sin(t * WIND_SPEED + phase) + 0.35 * sin(t * WIND_SPEED * 1.9 + phase * 2.7);
    // Bias the sway along a consistent breeze direction (x stronger than z).
    return vec2<f32>(wave, wave * 0.55) * WIND_STRENGTH;
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#endif

#ifdef VERTEX_POSITIONS
    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    var sway = 1.0;
#ifdef VERTEX_COLORS
    sway = vertex.color.a;
#endif
    let w = wind_offset(out.world_position.xz, globals.time) * sway;
    out.world_position = vec4<f32>(
        out.world_position.x + w.x,
        out.world_position.y,
        out.world_position.z + w.y,
        out.world_position.w,
    );
    out.position = position_world_to_clip(out.world_position.xyz);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif

    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Radial distance dither. `fade` goes 0 (near, all kept) → 1 (far, all
    // dropped); `uv.x` is a per-blade random so the blade is kept iff its random
    // is >= fade. Whole-blade decision → no partial blades, stable across frames.
    let dist = distance(in.world_position.xz, view.world_position.xz);
    let fade = clamp((dist - FADE_START) / (FADE_END - FADE_START), 0.0, 1.0);
#ifdef VERTEX_UVS_A
    if in.uv.x < fade {
        discard;
    }
#endif

    // Standard StandardMaterial PBR shading from here on.
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    // Stylized world-space colour patches: low-frequency fBm in world XZ tints
    // the albedo across the field (the reference shader's `mix(color, color2,
    // noise)`). The vertex-colour gradient is already in `base_color`; this
    // multiplies a per-region tint on top, so neighbouring tiles share patches
    // (no seams). Faint warm bias makes bright patches read a touch yellower.
    let n = fbm2(in.world_position.xz / NOISE_SCALE);
    let patch_level = mix(PATCH_MIN, PATCH_MAX, n);
    // Bias the patch tint green (lift G, trim R/B) so brighter patches stay
    // green rather than reading golden.
    let tint = vec3<f32>(patch_level * 0.96, patch_level * 1.03, patch_level * 0.96);
    pbr_input.material.base_color = vec4<f32>(
        pbr_input.material.base_color.rgb * tint,
        pbr_input.material.base_color.a,
    );

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
