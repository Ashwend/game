// Cel-shaded ore/vein material: a standalone toon `Material` for the ore-node
// boulders so the hand-painted rock texture reads anime/cartoon instead of being
// smoothly PBR-lit (which flattened the painted highlights). Per-mineral colour
// rides on the glb COLOR_0 (grey rock body vs bright mineral chunks); the rock
// detail texture is shared across all four ores.
//
// Like `terrain.wgsl` this is a STANDALONE `Material` (not an ExtendedMaterial):
// it owns the material bind group, so its bindings stay alive on Metal. The
// bindings use `@group(#{MATERIAL_BIND_GROUP})`, NOT a literal `@group(2)`: in
// Bevy 0.18 the per-object mesh array sits at the literal `@group(2)`
// (`mesh_bindings`) and the material group is 3 via that shaderdef; hardcoding
// `@group(2)` collides. We do our own quantised sun lighting (the toon look) and
// only borrow `main_pass_post_lighting_processing` so the ore still fogs like the
// rest of the scene. No `apply_pbr_lighting` -> no shadow-map read, same trade
// the grass shader makes (ore still CASTS shadows via the default prepass).

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types,
    pbr_types::{PbrInput, pbr_input_new},
    pbr_functions::{main_pass_post_lighting_processing, calculate_view, prepare_world_normal},
    mesh_bindings::mesh,
    mesh_view_bindings::{view, lights},
}

// Material bind group.
//   params.x = cel band count (fewer = harder steps)
//   params.y = ambient floor (flat fill, keeps the dark side / night readable)
//   params.z = ink-edge strength (dark silhouette outline; 0 = off)
//   params.w = ink-edge width exponent (smaller = wider edge)
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var rock_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var rock_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> params: vec4<f32>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Albedo: shared grey-rock detail * per-mineral COLOR_0 (linear * linear).
    let tex = textureSample(rock_tex, rock_samp, in.uv).rgb;
    let albedo = tex * in.color.rgb;

    // World normal, front-face corrected (same call terrain/grass use).
    let world_normal = prepare_world_normal(in.world_normal, false, is_front);
    let n = normalize(world_normal);

    // Sun = directional light 0 (same access the grass shader uses).
    let sun = lights.directional_lights[0];
    let l = normalize(sun.direction_to_light);

    // Half-Lambert wrap, then quantise into hard cel bands.
    let bands = max(params.x, 1.0);
    let wrap = dot(n, l) * 0.5 + 0.5;
    let stepped = floor(wrap * bands) / bands;       // terraced light

    // Quantised sun term + a flat ambient fill so the shadow side / night never
    // crush to pure black (the toon "ambient" band).
    let ambient = vec3<f32>(params.y, params.y, params.y);
    var rgb = albedo * (sun.color.rgb * stepped + ambient);

    // Dark ink-style silhouette edge: darken fragments whose normal turns away
    // from the camera, approximating a hand-drawn outline (reads more cartoon
    // than a bright rim light). params.z = strength, params.w = width exponent.
    let is_ortho = view.clip_from_view[3].w == 1.0;
    let v = calculate_view(in.world_position, is_ortho);
    let edge = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), max(params.w, 0.5));
    rgb = mix(rgb, rgb * 0.10, clamp(edge * params.z, 0.0, 1.0));

    // Minimal PbrInput so the fragment still fades into the scene distance fog,
    // mirroring terrain.wgsl's hand-built post-processing call.
    var pbr_input: PbrInput = pbr_input_new();
    pbr_input.flags = mesh[in.instance_index].flags;
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.material.flags = pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, vec4<f32>(rgb, 1.0));
    return out;
}
