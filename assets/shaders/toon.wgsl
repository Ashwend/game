// Shared cel-shaded (toon / anime) material: PBR-then-posterise. The prop is lit
// by the REAL engine lighting (`apply_pbr_lighting`: the same sun + atmosphere
// IBL + received shadows + day/night exposure the ground gets), then the lit
// luminance is quantised into a few hard cel bands so it still reads anime, with
// a dark ink silhouette edge on top. Going through real PBR is the whole point:
// it dims by illuminance/exposure at night like every other surface, so the prop
// no longer blows white after dark (the earlier hand-rolled lighting read the
// light colour but ignored its illuminance + the view exposure, so it stayed
// day-bright all night).
//
// Surface albedo = `detail_texture * COLOR_0`: the per-prop colour rides on the
// glb COLOR_0 and the detail texture adds grain (vertex-colour-only props bind a
// 1x1 white detail, so it reduces to COLOR_0).
//
// Standalone `Material` (not an ExtendedMaterial): it owns the material bind
// group so its bindings stay alive on Metal. Bindings use
// `@group(#{MATERIAL_BIND_GROUP})`, NOT a literal `@group(2)` (which collides
// with `mesh_bindings` on Bevy 0.18). We deliberately do NOT import
// `pbr_fragment` (it would pull a second `StandardMaterial` binding set into our
// material group); the `PbrInput` is hand-built, mirroring `terrain.wgsl`.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types,
    pbr_types::{PbrInput, pbr_input_new},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing, calculate_view, prepare_world_normal},
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}

// Material bind group.
//   params.x = cel band count (fewer = harder steps)
//   params.y = alpha-mask cutoff (0 = opaque/off; >0 = discard texture alpha below
//              it, for the double-sided grass-card tufts). Was the flat ambient
//              floor; PBR now supplies ambient via IBL.
//   params.z = ink-edge strength (dark silhouette outline; 0 = off)
//   params.w = ink-edge width exponent (smaller = wider edge)
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var detail_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var detail_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> params: vec4<f32>;
// Triplanar tiles/metre for the no-UV (deployable) path; unused by UV'd meshes.
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> tex_scale: f32;
// Per-instance opacity; 1.0 for static props, driven below 1.0 only by the
// tree-felling dissolve (the material's alpha_mode flips to Blend then).
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> fade: f32;

// Saturation lift applied after the cel posterise so the banded result keeps the
// bright, high-chroma anime feel instead of reading muted. 1.0 = off.
const TOON_SATURATION: f32 = 1.25;
// Cel posterise tuning. The lighting STRENGTH (albedo divided out) is quantised
// into hard bands, then `albedo * band` rebuilds the colour so every band keeps
// the prop's OWN hue (the old luminance-recolour kept the lit colour's hue, which
// on the shadow side is the desaturated sky ambient, reading as a washed flat
// tone). LIT_GAIN scales the bands so the brightest reaches ~full albedo by day.
// Below the lowest band the value follows the real shade * SHADOW_FILL instead of
// a flat floor: that keeps a *daytime* shadow side (lit by ambient IBL, moderate
// shade) dim-but-present while *night* (very low shade) still goes dark, so a
// side-lit prop/field doesn't crush its shadowed half to a flat near-black cliff.
const TOON_LIT_GAIN: f32 = 1.5;
const TOON_SHADOW_FILL: f32 = 0.6;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Albedo: detail texture * COLOR_0 (linear * linear). Both optional per mesh:
    // textured props (ore) carry UVs + COLOR_0; vertex-colour-only props
    // (deployables) have COLOR_0 but no UVs, so they triplanar-project instead.
    // Guarding the field accesses behind the mesh's own shaderdefs is required:
    // accessing `in.uv` on a mesh without TEXCOORD_0 fails to compile (the bug
    // that left the deployable bodies invisible while still casting a shadow).
#ifdef VERTEX_UVS_A
    // UV'd meshes (ore glbs, grass cards) sample the detail texture directly. The
    // grass-card tufts carry their blade silhouette in the texture ALPHA: when
    // params.y (the mask cutoff) is set, discard the transparent gaps so the card
    // reads as a cut-out tuft instead of an opaque rectangle. Opaque props leave
    // params.y at 0 so nothing is discarded.
    let tex_sample = textureSample(detail_tex, detail_samp, in.uv);
    let tex = tex_sample.rgb;
    if params.y > 0.0 && tex_sample.a < params.y {
        discard;
    }
#else
    // No mesh UVs (deployable props): world-space triplanar projection so the
    // detail texture wraps every face without an unwrap. Blend the three axis
    // projections by the (squared) world normal so edges don't smear.
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

    // Hand-built PbrInput (mirrors terrain.wgsl). Matte so the cel bands aren't
    // fought by a glossy Fresnel specular streak; the mesh flags carry the
    // shadow-receiver bit, so the prop takes tree / building shadows.
    var pbr_input: PbrInput = pbr_input_new();
    pbr_input.flags = mesh[in.instance_index].flags;
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.V = calculate_view(in.world_position, pbr_input.is_orthographic);
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = prepare_world_normal(in.world_normal, false, is_front);
    pbr_input.N = normalize(pbr_input.world_normal);
    pbr_input.material.base_color = vec4<f32>(albedo, 1.0);
    pbr_input.material.perceptual_roughness = 0.95;
    pbr_input.material.reflectance = vec3<f32>(0.0, 0.0, 0.0);
    pbr_input.material.metallic = 0.0;
    pbr_input.material.flags = pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;

    // Real PBR lighting (sun + atmosphere IBL + received shadows + exposure).
    let lit = apply_pbr_lighting(pbr_input);

    // Cel posterise that preserves the prop's hue (see consts above): divide the
    // albedo out to get the lighting STRENGTH, quantise that into hard bands, then
    // re-apply albedo. Keeps the rock's own colour in shadow (no washed flat tone)
    // and lets NIGHT_FLOOR / LIT_GAIN set the dark/bright ends.
    let lw = vec3<f32>(0.2126, 0.7152, 0.0722);
    let albedo_lum = max(dot(albedo, lw), 1e-3);
    let shade = clamp(dot(lit.rgb, lw) / albedo_lum, 0.0, 0.999);
    let bands = max(params.x, 2.0);
    let banded = clamp(floor(shade * bands) / bands * TOON_LIT_GAIN, 0.0, 1.0);
    let shade_q = max(banded, shade * TOON_SHADOW_FILL);
    var rgb = albedo * shade_q;

    // Dark ink-style silhouette edge: darken fragments whose normal turns away
    // from the camera, approximating a hand-drawn outline. params.z = strength,
    // params.w = width exponent.
    let edge = pow(1.0 - clamp(dot(pbr_input.N, pbr_input.V), 0.0, 1.0), max(params.w, 0.5));
    rgb = mix(rgb, rgb * 0.10, clamp(edge * params.z, 0.0, 1.0));

    // Saturation lift for the colourful anime feel (value is already correct from
    // the PBR pass, so no brightness gain, that would just blow the highlights).
    let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    rgb = max(mix(vec3<f32>(luma, luma, luma), rgb, TOON_SATURATION), vec3<f32>(0.0));

    // Output alpha: opaque props (and the felling dissolve) ride `lit.a * fade`.
    // Alpha-masked grass cards instead pass the texture's silhouette alpha through
    // so MSAA alpha-to-coverage can soften the cut-out edge (matches the detail
    // grass shader); `fade` stays 1.0 for them.
    var out_alpha = lit.a * fade;
#ifdef VERTEX_UVS_A
    if params.y > 0.0 {
        out_alpha = tex_sample.a * fade;
    }
#endif

    var out: FragmentOutput;
    // `fade` is 1.0 for every static prop; the felling dissolve lowers it so the
    // banded trunk/canopy fade out (its material is in the Blend pass by then).
    out.color = main_pass_post_lighting_processing(pbr_input, vec4<f32>(rgb, out_alpha));
    return out;
}
