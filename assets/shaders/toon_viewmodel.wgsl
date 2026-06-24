// Cel-shaded material for the FIRST-PERSON held tool (a camera-child viewmodel).
//
// Same look + bind group as `toon.wgsl`, but the cel BAND PATTERN is lit by a key
// light fixed in VIEW space, not the world sun. A held item's view-space normal is
// invariant to where the camera looks (its transform relative to the camera never
// changes), so a view-space key light gives rock-stable bands: they no longer
// swim/snap across the tool as you turn, the way world-sun cel banding does on a
// spinning viewmodel. This is the standard "viewmodel light rig" trick (FPS games
// fix a key light to the camera so the weapon shading stays put).
//
// Day/night is preserved WITHOUT reintroducing the swim: overall brightness comes
// from a single scene probe (real `apply_pbr_lighting` on a fixed up-facing white
// surface), which tracks the sun/exposure but does not depend on the tool's
// orientation. So the tool dims at night like the world yet keeps a stable,
// flattering cel ramp by day. Only the in-hand item uses this; the third-person
// tool on other players stays on the world `toon.wgsl` material.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types,
    pbr_types::{PbrInput, pbr_input_new},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing, calculate_view, prepare_world_normal},
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}

// Bind group identical to `ToonMaterial` (see toon.wgsl for the param packing).
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var detail_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var detail_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> params: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> tex_scale: f32;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> fade: f32;
// Developer debug bitfield (dev-only; 0 in shipped builds). Shared layout with
// toon.wgsl: 1=no posterize, 2=no band AA, 4=no ink, 8=no saturation.
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var<uniform> dev_flags: u32;
const DEV_NO_POSTERIZE: u32 = 1u;
const DEV_NO_BAND_AA: u32 = 2u;
const DEV_NO_INK: u32 = 4u;
const DEV_NO_SATURATION: u32 = 8u;

const TOON_SATURATION: f32 = 1.10;
// Fixed key-light direction in VIEW space (camera-relative). +X right, +Y up,
// +Z toward the viewer. Upper-right and slightly toward the camera reads as a
// flattering three-quarter key. Stable, so the cel bands never swim.
const VM_KEY_DIR: vec3<f32> = vec3<f32>(0.35, 0.55, 0.78);
// Shadow-side floor: the darkest cel band keeps this fraction of the lit value so
// the unlit side stays a readable cel tone instead of crushing to black. Lowered
// from 0.55 for more contrast, the high floor read flat/washed on the in-hand tool.
const VM_AMBIENT: f32 = 0.42;
// Overall brightness trim for the in-hand tool. The viewmodel camera carries no
// atmosphere IBL of its own, so the probe is lit by the bare daytime sun, which
// blew the tool (especially the light iron head) toward white. Scaling the scene
// strength dims it back into the frame while preserving day/night tracking.
const VM_SCENE_GAIN: f32 = 0.65;
// Softness of each band-step edge (in band units). Small => crisp cel steps that
// still anti-alias instead of a hard pixel cliff.
const VM_STEP_SOFT: f32 = 0.16;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Albedo = detail texture * COLOR_0, same as the world toon material.
#ifdef VERTEX_UVS_A
    let tex_sample = textureSample(detail_tex, detail_samp, in.uv);
    let tex = tex_sample.rgb;
    if params.y > 0.0 && tex_sample.a < params.y {
        discard;
    }
#else
    let wp = in.world_position.xyz * tex_scale;
    var bw = abs(normalize(in.world_normal));
    bw = bw * bw;
    bw = bw / max(bw.x + bw.y + bw.z, 1e-4);
    let tex = textureSample(detail_tex, detail_samp, wp.yz).rgb * bw.x
        + textureSample(detail_tex, detail_samp, wp.zx).rgb * bw.y
        + textureSample(detail_tex, detail_samp, wp.xy).rgb * bw.z;
#endif
#ifdef VERTEX_COLORS
    let vcol = in.color.rgb;
#else
    let vcol = vec3<f32>(1.0, 1.0, 1.0);
#endif
    let albedo = tex * vcol;

    let world_n = prepare_world_normal(in.world_normal, false, is_front);
    let n = normalize(world_n);
    let lw = vec3<f32>(0.2126, 0.7152, 0.0722);

    // --- Stable cel band pattern: light in VIEW space with a fixed key dir. ---
    let n_view = normalize((view.view_from_world * vec4<f32>(n, 0.0)).xyz);
    let key = normalize(VM_KEY_DIR);
    // Half-lambert keeps the shadow side from going fully dark and gives a softer
    // wrap; quantise into a few hard-but-soft-edged bands.
    let ndl = clamp(dot(n_view, key) * 0.5 + 0.5, 0.0, 1.0);
    let bands = max(params.x, 2.0);
    var band: f32;
    if (dev_flags & DEV_NO_POSTERIZE) != 0u {
        band = ndl;                                  // Dev: smooth, no bands.
    } else {
        let q = ndl * bands;
        let fl = floor(q);
        let frac = q - fl;
        if (dev_flags & DEV_NO_BAND_AA) != 0u {
            band = clamp(fl / bands, 0.0, 1.0);      // Dev: hard step, no soft edge.
        } else {
            band = clamp((fl + smoothstep(0.5 - VM_STEP_SOFT, 0.5 + VM_STEP_SOFT, frac)) / bands, 0.0, 1.0);
        }
    }
    let lit_strength = mix(VM_AMBIENT, 1.0, band);

    // --- Day/night brightness from one orientation-independent scene probe:
    // real PBR lighting on a fixed up-facing white surface at the tool's position.
    // Tracks sun + IBL + exposure but never depends on how the camera is turned,
    // so it adds no swim. ---
    var probe: PbrInput = pbr_input_new();
    probe.flags = mesh[in.instance_index].flags;
    probe.is_orthographic = view.clip_from_view[3].w == 1.0;
    probe.V = calculate_view(in.world_position, probe.is_orthographic);
    probe.frag_coord = in.position;
    probe.world_position = in.world_position;
    probe.world_normal = vec3<f32>(0.0, 1.0, 0.0);
    probe.N = vec3<f32>(0.0, 1.0, 0.0);
    probe.material.base_color = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    probe.material.perceptual_roughness = 1.0;
    probe.material.reflectance = vec3<f32>(0.0, 0.0, 0.0);
    probe.material.metallic = 0.0;
    probe.material.flags = pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    let probe_lit = apply_pbr_lighting(probe);
    // Scene strength for a white up-facing surface == the world toon material's
    // `shade` for that surface, so brightness matches the rest of the scene.
    let scene = max(dot(probe_lit.rgb, lw), 0.0) * VM_SCENE_GAIN;

    var rgb = albedo * lit_strength * scene;

    // Ink-edge silhouette in VIEW space (also stable): darken where the view normal
    // turns away from the camera. n_view.z points toward the viewer. (Dev-toggleable.)
    if (dev_flags & DEV_NO_INK) == 0u {
        let edge = pow(1.0 - clamp(n_view.z, 0.0, 1.0), max(params.w, 0.5));
        rgb = mix(rgb, rgb * 0.10, clamp(edge * params.z, 0.0, 1.0));
    }

    // Saturation lift for the anime feel (value unchanged).
    if (dev_flags & DEV_NO_SATURATION) == 0u {
        let luma = dot(rgb, lw);
        rgb = max(mix(vec3<f32>(luma, luma, luma), rgb, TOON_SATURATION), vec3<f32>(0.0));
    }

    // Fog + exposure + tonemap through the same post path the world uses, so the
    // viewmodel sits in the scene's brightness (the probe carries position/V/fog).
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(probe, vec4<f32>(rgb, fade));
    return out;
}
