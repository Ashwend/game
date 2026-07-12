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
// Developer debug bitfield (dev-only `Dev` options tab; 0 in shipped builds).
// Each set bit DISABLES a stage so a glitch can be isolated live. See
// `state::toon_dev_bits`: 1=no posterize, 2=no band AA, 4=no ink, 8=no saturation.
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var<uniform> dev_flags: u32;
const DEV_NO_POSTERIZE: u32 = 1u;
const DEV_NO_BAND_AA: u32 = 2u;
const DEV_NO_INK: u32 = 4u;
const DEV_NO_SATURATION: u32 = 8u;
// Self-illumination (night glow). `emissive_tex` is the glow mask (bright =
// glowing); `emissive.rgb` is the added glow colour, `emissive.a >= 0.5` gates
// the glow by COLOR_0 vertex alpha so one mesh can mix glowing crystals with a
// non-glowing body (the meteorite node). `emissive` is vec4(0) for every other
// cel prop, so this whole path is inert (existing ore is untouched).
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var emissive_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var emissive_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(8) var<uniform> emissive: vec4<f32>;

// Saturation lift applied after the cel posterise so the banded result keeps a
// gentle anime chroma without tipping into the oversaturated/candy look. 1.0 = off.
const TOON_SATURATION: f32 = 1.10;
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
// Cel "crispness" gate (mirrors the grass shader). On a large FLAT prop face the
// cel `shade * TOON_SHADOW_FILL` floor crushes a shadow-lit side to near-black (a
// deployable read as a black blob at a low dawn sun, where the atmosphere IBL is
// dim and there is no direct sun on that face). A flat face has a near-uniform
// normal, so its lighting gradient `fwidth(shade)` is ~0; a curved/faceted form
// has a healthy gradient. Fade the cel toward smooth (`shade * TOON_LIT_GAIN`,
// ~2.5x brighter than the crush floor) only on the flat, low-gradient faces, so
// the dark side reads with form instead of a silhouette. This is a NO-OP on
// curved ore boulders, faceted canopy, and cylindrical trunks (their fwidth keeps
// cel_strength ~1), verified: their before/after delta sat below the frame noise.
const TOON_CEL_FLAT: f32 = 0.004;
const TOON_CEL_DETAIL: f32 = 0.020;

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
    let smooth_q = clamp(shade * TOON_LIT_GAIN, 0.0, 1.0);
    var shade_q: f32;
    if (dev_flags & DEV_NO_POSTERIZE) != 0u {
        // Dev: posterize OFF -> smooth lighting (same exposure as the banded path).
        shade_q = smooth_q;
    } else {
        let q = shade * bands;
        var band_aa: f32;
        if (dev_flags & DEV_NO_BAND_AA) != 0u {
            // Dev: hard floor(), no edge AA (reproduces the old crawling-band look).
            band_aa = floor(q);
        } else {
            // Anti-aliased cel step (replaces a bare floor()): quantise the lighting
            // into hard bands, but soften each band BOUNDARY by the screen-space
            // gradient of the lit value (fwidth). On a clean gradient the edge stays
            // ~1px crisp, so the bands read exactly as before; where the RECEIVED
            // shadow edge is noisy (PCSS penumbra + self-shadow acne) fwidth widens
            // and the boundary dissolves instead of snapping a whole region between
            // two bands frame-to-frame. That snap is the crawling shadow band on a
            // tree trunk; this removes it without losing a band. The `q - 0.5` centres
            // the smoothstep on each original floor() step, so band positions /
            // brightness are unchanged away from the noisy edges.
            let aa = max(fwidth(q) * 0.5, 0.02);
            band_aa = floor(q - 0.5) + smoothstep(0.5 - aa, 0.5 + aa, fract(q - 0.5));
        }
        let banded = clamp(band_aa / bands * TOON_LIT_GAIN, 0.0, 1.0);
        let cel_q = max(banded, shade * TOON_SHADOW_FILL);
        // Fade the cel toward smooth on flat, low-gradient faces (see consts): keeps
        // every curved/faceted prop fully cel, but stops a flat shadow face crushing
        // to a black blob. Skipped in the band-AA-off dev mode so that toggle still
        // shows the raw hard floor() stepping.
        if (dev_flags & DEV_NO_BAND_AA) == 0u {
            let cel_strength = smoothstep(TOON_CEL_FLAT, TOON_CEL_DETAIL, fwidth(shade));
            shade_q = mix(smooth_q, cel_q, cel_strength);
        } else {
            shade_q = cel_q;
        }
    }
    var rgb = albedo * shade_q;

    // Dark ink-style silhouette edge: darken fragments whose normal turns away
    // from the camera, approximating a hand-drawn outline. params.z = strength,
    // params.w = width exponent. (Dev-toggleable.)
    if (dev_flags & DEV_NO_INK) == 0u {
        let edge = pow(1.0 - clamp(dot(pbr_input.N, pbr_input.V), 0.0, 1.0), max(params.w, 0.5));
        rgb = mix(rgb, rgb * 0.10, clamp(edge * params.z, 0.0, 1.0));
    }

    // Saturation lift for the colourful anime feel (value is already correct from
    // the PBR pass, so no brightness gain, that would just blow the highlights).
    if (dev_flags & DEV_NO_SATURATION) == 0u {
        let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        rgb = max(mix(vec3<f32>(luma, luma, luma), rgb, TOON_SATURATION), vec3<f32>(0.0));
    }

    // Night-glow: ADD an emissive term on top of the cel-lit surface. Added after
    // the day/night-exposed cel term (not multiplied into lighting), so a crystal
    // stays visibly lit in the dark for the exploration read, while in daylight
    // the bright surround keeps it reading as a glowing crystal rather than a
    // blown-out blob. `emissive` is vec4(0) for every non-ember prop, so this is a
    // no-op that leaves the existing ore untouched.
    if emissive.r > 0.0 || emissive.g > 0.0 || emissive.b > 0.0 {
        // The mask texture is a sparse vein pattern (mostly black): use it to
        // BOOST bright veins, not to gate the glow, or the crystal would be dark
        // almost everywhere. A baseline floor makes the whole crystal glow (the
        // exploration read: visible from far at night), with the mask adding hot
        // veins on top.
#ifdef VERTEX_UVS_A
        let vein = textureSample(emissive_tex, emissive_samp, in.uv).r;
#else
        let vein = 0.0;
#endif
        let glow_amount = 0.55 + 0.45 * vein;
        // Gate by COLOR_0 vertex alpha when emissive.a is set, so the meteorite
        // slag body (alpha 0) stays dark while its crystals (alpha 1) glow.
#ifdef VERTEX_COLORS
        let glow_gate = select(1.0, in.color.a, emissive.a >= 0.5);
#else
        let glow_gate = select(1.0, 0.0, emissive.a >= 0.5);
#endif
        rgb = rgb + emissive.rgb * glow_amount * glow_gate;
    }

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
