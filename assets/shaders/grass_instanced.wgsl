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
//   * vertex wind: a three-layer model (long-wavelength gust band that rolls
//     across the field + mid sway + tip flutter), weighted by vertex-colour
//     alpha so the blade bends in an arc with a pinned root, droops as it leans
//     so it lays over instead of stretching, and re-points its normal up at the
//     tip (see `wind_offset`),
//   * fragment radial dither (whole blades drop out with distance: a stable
//     per-instance key vs a camera-distance fade, no hard edge / tile seam),
//   * world-space fBm colour patches (the hand-painted "patchy lawn" look).

#import bevy_pbr::{
    mesh_view_bindings::{view, globals},
    view_transformations::position_world_to_clip,
    pbr_types::{PbrInput, pbr_input_new, STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing, calculate_view},
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
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
    // b = [yaw, shade, _, _]
    @location(4) i_b: vec4<f32>,
    // c = [tint_r, tint_g, tint_b, _]  per-blade biome colour tint
    @location(6) i_c: vec4<f32>,
}

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    // Card texture coordinates; the fragment samples the grass-tuft texture for
    // both the blade colour gradient and the alpha silhouette.
    @location(3) uv: vec2<f32>,
    // Stable per-card key (0..1) for the distance dither dissolve.
    @location(4) thin_key: f32,
    // World-space patch brightness (0..1 fBm), evaluated once per blade in the
    // vertex stage. It varies on a NOISE_SCALE (~14 m) cell, far coarser than a
    // blade card, so a per-blade value is indistinguishable from the old
    // per-fragment eval and saves a 3-octave fBm (~12 hash calls) per fragment.
    // (Named `patch_tint`, not `patch`: `patch` is a reserved WGSL keyword.)
    @location(5) patch_tint: f32,
}

// Interleaved gradient noise (Jimenez): a cheap, temporally-stable screen-space
// dither used to stipple the distance dissolve.
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// group(3): the grass-card tuft texture + sampler (the blade detail + silhouette).
// Day/night no longer needs a uniform here: the fragment lights via real PBR
// (apply_pbr_lighting), which dims by the scene's illuminance/exposure after dark
// like every other surface, so the old hand-fed `grass_day` blend is gone.
@group(3) @binding(0) var grass_tex: texture_2d<f32>;
@group(3) @binding(1) var grass_samp: sampler;
// Developer debug bitfield (dev-only `Dev` options tab; 0 in shipped builds).
// `.x` bits: 1 = disable cel posterize, 2 = disable wind. See `state::grass_dev_bits`.
@group(3) @binding(2) var<uniform> grass_dev: vec4<u32>;
const GRASS_DEV_NO_CEL: u32 = 1u;
const GRASS_DEV_NO_WIND: u32 = 2u;

// Alpha cutoff for the card silhouette. A discard below this keeps the cards
// readable under MSAA-off (FXAA); alpha-to-coverage refines the edge under MSAA.
const ALPHA_CUTOFF: f32 = 0.28;

// Distance dissolve window (metres from camera): full density before `start`,
// fully dithered out by `end` (just inside GRASS_RADIUS_M). A stable per-card key
// vs a screen-space dither gives a gradual stippled fade-out over this band, so
// the field tapers into the distance with no hard "grass line" and no card
// pop-in, and it works under FXAA (no alpha blend needed).
const FADE_START: f32 = 26.0;
const FADE_END: f32 = 50.0;
// Width (in key space) of the dither dissolve band; wider = softer transition.
const DITHER_BAND: f32 = 0.9;

// Three-layer wind model (see `wind_offset`): a dominant long-wavelength GUST
// band that sweeps across the whole field, a mid SWAY that keeps tufts alive,
// and a high-freq tip FLUTTER. All distances are in metres of horizontal tip
// displacement; speeds are rad/s; scales are spatial frequency (rad/m, lower =
// longer wavelength).

// Wind travel direction in world XZ (unit). The gust bands roll along this axis;
// because the phase is a function of world position the bands stay continuous
// across streamed tile seams.
const WIND_DIR: vec2<f32> = vec2<f32>(0.80, 0.60);

// Layer 1: the rolling GUST band. A long wavelength (so a whole swath leans
// together) on a slow temporal sweep, with its phase + amplitude wandered by
// low-frequency noise so it reads as a gust rather than clean "corduroy". This
// is the dominant motion and the thing that makes the field look windy.
const GUST_WAVELENGTH: f32 = 22.0;   // metres per band
const GUST_SPEED: f32 = 0.8;         // band sweep speed
const GUST_STRENGTH: f32 = 0.10;     // peak horizontal tip lean (m)
const GUST_BIAS: f32 = 0.30;         // net downwind lean baked into the oscillation
const GUST_NOISE_SCALE: f32 = 0.045; // 1/m, low-freq gust wander
const GUST_NOISE_PHASE: f32 = 1.6;   // how hard the noise warps the band phase

// Layer 2: mid SWAY, the original travelling wave, now a subtle secondary on top
// of the gust so individual tufts still breathe.
const WIND_STRENGTH: f32 = 0.03;
const WIND_SPEED: f32 = 1.15;
const WIND_SCALE: f32 = 0.09;

// Layer 3: tip FLUTTER, a high-freq tiny buzz confined to the very blade tips.
const FLUTTER_STRENGTH: f32 = 0.012;
const FLUTTER_SPEED: f32 = 7.0;
const FLUTTER_SCALE: f32 = 1.1;

// How much a bent blade shortens (tip drops) as it leans, so a hard gust lays
// the blade over instead of stretching it. ~0.5 keeps the blade length roughly
// constant for the lean amounts above.
const BEND_DROOP: f32 = 0.40;

// Subtle world-space brightness variation. `NOISE_SCALE` = metres per noise cell;
// the albedo is multiplied by a *neutral* (hueless) factor from `PATCH_MIN` to
// `PATCH_MAX` so tufts vary gently in lightness without an acidic colour shift.
const NOISE_SCALE: f32 = 14.0;
const PATCH_MIN: f32 = 0.80;
const PATCH_MAX: f32 = 1.18;

// Root->tip ambient occlusion baked into the blade albedo: a gentle contact
// shade near the ground that lifts toward the tip. (The old painterly sun /
// subsurface / tip-glow / night-fill terms are gone, the fragment now lights via
// real PBR + a cel posterise; see the fragment shader.)
const AO_ROOT: f32 = 0.75;       // brightness at the blade root (1 = no AO; gentle)
const AO_POW: f32 = 1.3;         // how fast AO lifts toward the tip
// Quantise the PBR-lit luminance into this many hard cel steps so the field
// reads toon-shaded (matches the ore/deployable ToonMaterial), not smoothly lit.
const GRASS_CEL_BANDS: f32 = 3.0;
// Cel posterise tuning (matches the ToonMaterial): the lighting STRENGTH (albedo
// divided out) is banded then re-applied as `albedo * band` so each band keeps
// the blade's own green. LIT_GAIN lifts the brightest band toward full albedo by
// day. Below the lowest band the value follows the real shade * SHADOW_FILL
// instead of a flat floor, so a daytime shadow side (ambient-lit, moderate shade)
// stays dim-but-present while night (very low shade) goes dark, no near-black
// cliff on a side-lit field.
const GRASS_LIT_GAIN: f32 = 1.5;
const GRASS_SHADOW_FILL: f32 = 0.6;
// Cel "crispness" gate (see the fragment): the per-pixel lighting gradient
// `fwidth(shade)` below which the cel posterise fades to smooth shading, and
// above which it is fully banded. Flat far field / uniform dim light sits below
// FLAT (renders smooth, no posterisation split); the detailed, contrasty near
// field sits above DETAIL (renders cel). Tuned so the dawn "field split" is gone
// but near-field tufts still read toon in daylight.
const GRASS_CEL_FLAT: f32 = 0.004;
const GRASS_CEL_DETAIL: f32 = 0.020;

// Tuft atlas layout (assets/textures/grass_atlas.png): a 3x2 grid of 6 toony
// tuft variants. Each blade's instance carries a cell index (`i_b.z`, 0..5); the
// vertex shader remaps the card's [0,1] UV into that cell so the field draws all
// six tufts (~1/6 each) instead of one repeated card. Their differing heights
// give the field free size variation.
const ATLAS_COLS: f32 = 3.0;
const ATLAS_ROWS: f32 = 2.0;

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

// World-space horizontal wind displacement for a point on a blade, already
// weighted by the baked sway weight `sway` (vertex-colour alpha: 0 root, 1 tip).
//
// Three layers, summed:
//   1. GUST band, a long-wavelength travelling wave along `WIND_DIR` whose phase
//      and amplitude are wandered by low-freq fBm so a whole swath leans and
//      recovers together, the rolling band. Dominant.
//   2. SWAY, the original mid-frequency travelling wave, now a subtle secondary.
//   3. FLUTTER, a high-freq buzz confined to the tips.
//
// Layers 1+2 are weighted by `pow(sway, 1.5)` so the bend concentrates up the
// blade (an arc with a pinned root, not a slide); the flutter is gated by
// `pow(sway, 4)` so only the tip jitters. The phase depends only on world
// position + time (no per-blade jitter), so bands stay coherent across tiles.
fn wind_offset(world_xz: vec2<f32>, t: f32, sway: f32) -> vec2<f32> {
    let dir = WIND_DIR;
    // Distance along the wind axis (metres); the gust band travels along this.
    let axis = dot(world_xz, dir);

    // Low-frequency gust wander: slowly-scrolling noise warps the band's phase
    // and amplitude so it never reads as a perfect repeating sine.
    let nz = fbm2(world_xz * GUST_NOISE_SCALE + vec2<f32>(t * 0.05, t * 0.03));

    // Layer 1: rolling gust band.
    let gust_k = 6.28318530718 / GUST_WAVELENGTH;
    let gust_phase = axis * gust_k - t * GUST_SPEED + (nz - 0.5) * GUST_NOISE_PHASE;
    let gust = (GUST_BIAS + sin(gust_phase)) * (0.7 + 0.6 * nz);
    var offset = dir * gust * GUST_STRENGTH;

    // Layer 2: mid sway (biased x>z so it leans along a consistent breeze).
    let p = world_xz.x * WIND_SCALE + world_xz.y * (WIND_SCALE * 0.7);
    let sway2 = sin(t * WIND_SPEED + p) + 0.35 * sin(t * WIND_SPEED * 1.9 + p * 2.7);
    offset += vec2<f32>(sway2, sway2 * 0.55) * WIND_STRENGTH;

    // Arc concentration: weight the bend toward the upper blade.
    offset *= pow(sway, 1.5);

    // Layer 3: tip flutter, confined to the very tips.
    let fphase = dot(world_xz, vec2<f32>(0.7, -0.7)) * FLUTTER_SCALE + t * FLUTTER_SPEED;
    let flutter = sin(fphase) * FLUTTER_STRENGTH * pow(sway, 4.0);
    offset += vec2<f32>(flutter, flutter * 0.5);

    return offset;
}

@vertex
fn vertex(v: Vertex) -> VsOut {
    let height_scale = v.i_a.w;
    let yaw = v.i_b.x;
    let shade = v.i_b.y;

    // Rotate the card about +Y by `yaw`, scale, then place at its world root.
    let s = sin(yaw);
    let c = cos(yaw);
    let p = v.position * height_scale;
    let rotated = vec3<f32>(c * p.x + s * p.z, p.y, -s * p.x + c * p.z);
    var world = vec3<f32>(v.i_a.x, v.i_a.z, v.i_a.y) + rotated;

    // Wind sway. `wind_offset` already weights the bend by the baked sway weight
    // (vertex-colour alpha) and concentrates it up the blade, so the blade curves
    // with a pinned root instead of sliding sideways.
    var w = vec2<f32>(0.0, 0.0);
    if (grass_dev.x & GRASS_DEV_NO_WIND) == 0u {
        w = wind_offset(world.xz, globals.time, v.color.a);
    }
    world.x += w.x;
    world.z += w.y;
    // A hard-bent blade shortens (tip drops) rather than stretching, so a strong
    // gust reads as the blade laying over, not a rubber band.
    world.y -= length(w) * v.color.a * BEND_DROOP;

    let n = v.normal;
    var world_normal = vec3<f32>(c * n.x + s * n.z, n.y, -s * n.x + c * n.z);
    // Blend the normal toward straight-up at the tip: bent tips stay lit soft
    // from above (matches the lit-from-above art direction) and it damps the
    // specular shimmer that moving thin blades otherwise throw.
    world_normal = normalize(mix(world_normal, vec3<f32>(0.0, 1.0, 0.0), pow(v.color.a, 2.0)));

    // Per-blade colour: the baked neutral-green gradient, graded by the per-blade
    // biome tint (`i_c`, set in world space by `tile_world_instances`) and a
    // per-blade brightness jitter (`shade`). Forest/plains/rocky/ore each pull the
    // green toward the local ground tone so the field matches the biome.
    var rgb = v.color.rgb * v.i_c.rgb * shade;
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));

    var out: VsOut;
    out.clip_position = position_world_to_clip(world);
    out.world_position = world;
    out.world_normal = normalize(world_normal);
    out.color = vec4<f32>(rgb, v.color.a);
    // Atlas cell remap: i_b.z in 0..5 -> (col, row) of the 3x2 tuft atlas, then
    // fit the card's [0,1] UV inside that cell so this blade draws its variant.
    let cell = v.i_b.z;
    let col = cell - floor(cell / ATLAS_COLS) * ATLAS_COLS;
    let row = floor(cell / ATLAS_COLS);
    out.uv = (v.uv + vec2<f32>(col, row)) / vec2<f32>(ATLAS_COLS, ATLAS_ROWS);
    out.thin_key = v.i_b.w; // stable per-card key for the distance dither
    // Hueless world-space patch brightness, evaluated from the blade's ROOT world
    // position (pre-wind, so the patch doesn't swim as the blade sways). Moved off
    // the fragment path: it's a ~14 m-scale field, so per-blade is plenty.
    let root_xz = vec2<f32>(v.i_a.x, v.i_a.y);
    out.patch_tint = fbm2(root_xz / NOISE_SCALE);
    return out;
}

@fragment
fn fragment(in: VsOut) -> @location(0) vec4<f32> {
    let dist = distance(in.world_position.xz, view.world_position.xz);

    // Distance dissolve (FXAA-safe): a stable per-card key vs a screen-space dither
    // ramped by distance, so cards stipple-dissolve out far away and dissolve in
    // smoothly as the camera approaches, instead of popping/"spawning". Done before
    // the texture + lighting so dithered-out far cards cost almost nothing. The
    // threshold is scaled so `near == 1` keeps every card (no near thinning).
    let near = 1.0 - smoothstep(FADE_START, FADE_END, dist);
    let threshold = near * (1.0 + DITHER_BAND) - DITHER_BAND * ign(in.clip_position.xy);
    if in.thin_key > threshold {
        discard;
    }

    // Sample the grass-tuft card texture: rgb is the painted blade gradient, alpha
    // is the tuft silhouette (mipmapped, so far cards resolve to a soft mass). A
    // low-threshold discard keeps it readable under MSAA-off; alpha-to-coverage
    // refines the edge under MSAA.
    let tex = textureSample(grass_tex, grass_samp, in.uv);
    if tex.a < ALPHA_CUTOFF {
        discard;
    }

    // Subtle, hueless world-space brightness variation (the fBm patch field is
    // evaluated once per blade in the vertex stage and interpolated in as `patch`).
    let tint = vec3<f32>(mix(PATCH_MIN, PATCH_MAX, in.patch_tint));

    // Height fraction up the card (0 root, 1 top): the vertex-colour alpha doubles
    // as the height ramp for AO, tip glow, and translucency.
    let height_frac = in.color.a;
    // Root->tip ambient occlusion: a gentle contact shade near the ground.
    let ao = mix(AO_ROOT, 1.0, pow(height_frac, AO_POW));

    // Albedo = the painted texture gradient × the per-blade biome tint+brightness
    // (in.color.rgb) × the world-space patch tint × AO.
    let albedo = tex.rgb * in.color.rgb * tint * ao;

    // Flatten the normal toward +Y with distance so far cards catch a uniform soft
    // top-light instead of shimmering as sub-pixel cards churn. Near cards keep
    // their up-biased blade normal (set in the vertex stage).
    let n_flat = clamp((dist - 12.0) / 30.0, 0.0, 0.6);
    let n = normalize(mix(normalize(in.world_normal), vec3<f32>(0.0, 1.0, 0.0), n_flat));

    // Real PBR lighting (PBR-then-posterise, same as the ore/deployable
    // ToonMaterial): light the blade with the engine's actual sun + atmosphere
    // IBL + RECEIVED shadows + day/night exposure, then quantise. This replaces
    // the old cheap half-Lambert + hand-rolled day/night fade (which read the sun
    // colour but not its illuminance/exposure, so it had to be manually faded out
    // at dusk to stop the navigation-bright moon blooming the field white). PBR
    // dims the field correctly after dark for free, and the field now takes tree /
    // building shadows. Matte so no glossy specular streak fights the cel bands.
    var pbr_input = pbr_input_new();
    pbr_input.flags = MESH_FLAGS_SHADOW_RECEIVER_BIT;
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.V = calculate_view(vec4<f32>(in.world_position, 1.0), pbr_input.is_orthographic);
    pbr_input.frag_coord = in.clip_position;
    pbr_input.world_position = vec4<f32>(in.world_position, 1.0);
    pbr_input.world_normal = n;
    pbr_input.N = n;
    pbr_input.material.base_color = vec4<f32>(albedo, 1.0);
    pbr_input.material.perceptual_roughness = 1.0;
    pbr_input.material.reflectance = vec3<f32>(0.0, 0.0, 0.0);
    pbr_input.material.metallic = 0.0;
    pbr_input.material.flags = STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    let lit = apply_pbr_lighting(pbr_input);

    // Cel posterise that preserves the blade's green (matches the ToonMaterial):
    // divide the albedo out to get the lighting STRENGTH, band that, then re-apply
    // albedo. NIGHT_FLOOR keeps the darkest band genuinely dim at night.
    let lw = vec3<f32>(0.2126, 0.7152, 0.0722);
    let albedo_lum = max(dot(albedo, lw), 1e-3);
    let shade = clamp(dot(lit.rgb, lw) / albedo_lum, 0.0, 0.999);
    // Anti-aliased cel step, same fwidth-softened boundary as toon.wgsl: keeps the
    // hard bands on clean gradients but dissolves the boundary where the received
    // shadow edge is noisy, so the field doesn't crawl along band edges.
    let smooth_q = clamp(shade * GRASS_LIT_GAIN, 0.0, 1.0);
    var shade_q: f32;
    if (grass_dev.x & GRASS_DEV_NO_CEL) != 0u {
        // Dev: cel posterize off -> smooth lighting, same exposure.
        shade_q = smooth_q;
    } else {
        let gq = shade * GRASS_CEL_BANDS;
        let gaa = max(fwidth(gq) * 0.5, 0.02);
        let gband = floor(gq - 0.5) + smoothstep(0.5 - gaa, 0.5 + gaa, fract(gq - 0.5));
        let banded = clamp(gband / GRASS_CEL_BANDS * GRASS_LIT_GAIN, 0.0, 1.0);
        let cel_q = max(banded, shade * GRASS_SHADOW_FILL);
        // Posterising a smooth, near-flat lighting gradient (the whole field under a
        // low dawn sun) snaps it into two flat plateaus with a visible tonal "split"
        // where the brightness crosses a band edge. The fwidth band-AA above only
        // softens that boundary by a few pixels, which does nothing when the gradient
        // is shallow (the plateaus on either side still read as two field halves). So
        // fade the cel out toward smooth exactly where the lighting has no real
        // per-pixel variation: `fwidth(shade)` is ~0 on the flat far field / uniform
        // dim light and healthy on the contrasty, detailed near field, so the cel
        // stays crisp where it reads as anime tufts and dissolves where it would read
        // as a posterisation artifact. This is time-of-day robust (not a 07:00 hack):
        // any flat, low-contrast lighting renders smooth, any varied lighting cels.
        let cel_strength = smoothstep(GRASS_CEL_FLAT, GRASS_CEL_DETAIL, fwidth(shade));
        shade_q = mix(smooth_q, cel_q, cel_strength);
    }
    let rgb = albedo * shade_q;

    let out_color = main_pass_post_lighting_processing(pbr_input, vec4<f32>(rgb, 1.0));

    // Alpha = the texture's soft silhouette only (distance is handled by the dither
    // dissolve above). Under MSAA this drives alpha-to-coverage for soft edges;
    // under FXAA it's ignored (opaque) and the texture cutout shapes the card.
    return vec4<f32>(out_color.rgb, tex.a);
}
