// GPU-instanced detail-grass shader (see `src/app/scene/grass/instancing.rs`).
//
// One shared blade mesh is drawn once per tile with a per-blade instance buffer.
// Each instance carries its world position, a yaw spin, height scale, a
// shade/warm colour jitter, and a stable dither key. The pipeline specialises
// off Bevy's `MeshPipeline`, so this shader has the mesh-view bind groups
// (lights, shadows, globals, atmosphere IBL) available; it hand-builds a
// `PbrInput` and calls `apply_pbr_lighting`, so instanced grass is lit by the
// exact same sun + atmosphere as the rest of the scene without a material bind
// group of its own.
//
// Effects layered on top (ported from the old baked `grass.wgsl`):
//   * vertex wind sway (world-space wave, weighted by vertex-colour alpha so
//     tips bend and roots stay planted, plus a per-instance phase),
//   * fragment radial dither (whole blades drop out with distance: a stable
//     per-instance key vs a camera-distance fade, no hard edge / tile seam),
//   * world-space fBm colour patches (the hand-painted "patchy lawn" look).

#import bevy_pbr::{
    mesh_view_bindings::{view, globals},
    view_transformations::position_world_to_clip,
    pbr_types::pbr_input_new,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing, calculate_view},
}

// Mesh attributes sit at their canonical locations (Position 0, Normal 1, UV 2,
// Color 5); UV_1 (3) and Tangent (4) are absent from the blade mesh, so the
// instance-step attributes take locations 3 and 4.
struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(5) color: vec4<f32>,
    // a = [world_x, world_z, base_y, height_scale]
    @location(3) i_a: vec4<f32>,
    // b = [yaw, shade, warm, dither]
    @location(4) i_b: vec4<f32>,
}

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) dither: f32,
}

// Radial-dither fade window (metres from camera): full density before `start`,
// fully thinned by `end`, just inside the tile despawn radius.
const FADE_START: f32 = 20.0;
const FADE_END: f32 = 46.0;

// Wind tuning. Strength = horizontal tip sway in metres; speed scales the
// temporal wave; scale = spatial frequency (radians/metre).
const WIND_STRENGTH: f32 = 0.05;
const WIND_SPEED: f32 = 1.15;
const WIND_SCALE: f32 = 0.09;

// Subtle world-space brightness variation. `NOISE_SCALE` = metres per noise cell;
// the albedo is multiplied by a *neutral* (hueless) factor from `PATCH_MIN` to
// `PATCH_MAX` so tufts vary gently in lightness without an acidic colour shift.
const NOISE_SCALE: f32 = 14.0;
const PATCH_MIN: f32 = 0.88;
const PATCH_MAX: f32 = 1.08;

// World-space horizontal wind at a point (before the per-vertex sway weight). The
// phase depends ONLY on world position, so a whole tuft and its neighbours sway
// together as one rolling gust (no per-blade phase, that made each straw move
// independently like creature arms). A faster, smaller secondary wave keeps it
// from looking like a single clean sine.
fn wind_offset(world_xz: vec2<f32>, t: f32) -> vec2<f32> {
    let p = world_xz.x * WIND_SCALE + world_xz.y * (WIND_SCALE * 0.7);
    let wave = sin(t * WIND_SPEED + p) + 0.35 * sin(t * WIND_SPEED * 1.9 + p * 2.7);
    return vec2<f32>(wave, wave * 0.55) * WIND_STRENGTH;
}

// --- Procedural value noise (binding-free). Dave Hoskins hash + bilinear value
// noise + 3-octave fBm. ---
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

@vertex
fn vertex(v: Vertex) -> VsOut {
    let height_scale = v.i_a.w;
    let yaw = v.i_b.x;
    let shade = v.i_b.y;
    let warm = v.i_b.z;
    let dither = v.i_b.w;

    // Rotate the blade about +Y by `yaw`, scale, then place at its world root.
    let s = sin(yaw);
    let c = cos(yaw);
    let p = v.position * height_scale;
    let rotated = vec3<f32>(c * p.x + s * p.z, p.y, -s * p.x + c * p.z);
    var world = vec3<f32>(v.i_a.x, v.i_a.z, v.i_a.y) + rotated;

    // Wind sway, weighted by the baked sway weight (vertex-colour alpha: 0 root,
    // 1 tip), so blades bend rather than slide.
    let w = wind_offset(world.xz, globals.time) * v.color.a;
    world.x += w.x;
    world.z += w.y;

    let n = v.normal;
    let world_normal = vec3<f32>(c * n.x + s * n.z, n.y, -s * n.x + c * n.z);

    // Per-instance shade/warm jitter on the baked base->tip gradient (mirrors the
    // CPU `grass_blade_colors`: darken-only shade + warm pushes red up, blue down).
    // The blade's base colour is already slightly dried (see `build_instanced_blade_mesh`)
    // so one uniform green sits on both forest and plains ground.
    var rgb = v.color.rgb * shade
        + vec3<f32>(warm * 0.05, warm * 0.01, warm * -0.03);
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));

    var out: VsOut;
    out.clip_position = position_world_to_clip(world);
    out.world_position = world;
    out.world_normal = normalize(world_normal);
    out.color = vec4<f32>(rgb, v.color.a);
    out.dither = dither;
    return out;
}

@fragment
fn fragment(in: VsOut) -> @location(0) vec4<f32> {
    // Radial distance dither: keep the blade iff its stable key is >= the fade.
    let dist = distance(in.world_position.xz, view.world_position.xz);
    let fade = clamp((dist - FADE_START) / (FADE_END - FADE_START), 0.0, 1.0);
    if in.dither < fade {
        discard;
    }

    // Subtle, hueless world-space brightness variation.
    let np = fbm2(in.world_position.xz / NOISE_SCALE);
    let tint = vec3<f32>(mix(PATCH_MIN, PATCH_MAX, np));

    // Hand-built PBR so grass is lit by the scene's sun + atmosphere IBL.
    var pbr_input = pbr_input_new();
    pbr_input.frag_coord = in.clip_position;
    pbr_input.world_position = vec4<f32>(in.world_position, 1.0);
    let n = normalize(in.world_normal);
    pbr_input.world_normal = n;
    pbr_input.N = n;
    pbr_input.V = calculate_view(pbr_input.world_position, pbr_input.is_orthographic);
    pbr_input.material.base_color = vec4<f32>(in.color.rgb * tint, 1.0);
    pbr_input.material.perceptual_roughness = 0.95;
    pbr_input.material.metallic = 0.0;

    var out_color = apply_pbr_lighting(pbr_input);
    out_color = main_pass_post_lighting_processing(pbr_input, out_color);
    return out_color;
}
