// Prepass (depth + motion-vector + normal) fragment for `ToonMaterial`. Its only
// job beyond the stock prepass is the ALPHA-MASK DISCARD.
//
// The grass-card tufts (params.y > 0, the hay/tall grass) carry their silhouette
// in the detail texture's alpha. When TAA is on, Bevy runs a depth + motion
// prepass; the *default* prepass fragment has no material info, so it writes the
// whole opaque card quad into the depth buffer. That occludes the ground behind
// the card's transparent area, and the main pass then discards the grass there,
// leaving black card-shaped holes under TAA. Discarding the same transparent
// texels here keeps the prepass in lockstep with the main pass (toon.wgsl).
//
// Opaque toon props (ore / trees / deployables) have params.y == 0, so the
// discard never fires for them; this shader is a pass-through. Deployables have
// no UVs (`VERTEX_UVS_A` undefined) and skip the sample entirely.
//
// Overriding `prepass_fragment_shader` replaces only the fragment stage, so the
// stock prepass vertex still fills `VertexOutput`; this mirrors that vertex's
// normal + motion-vector outputs (see `bevy_pbr::prepass`).
//
// `prepass_io::FragmentOutput` only exists when a fragment-writing prepass is
// active (`PREPASS_FRAGMENT`: normal / motion / deferred). A DEPTH-ONLY prepass
// (e.g. the shadow-map pass that the opaque ore/tree/deployable props run) has
// no `FragmentOutput`, so referencing it there fails to compile
// (`unknown type ... FragmentOutput`). But because `ToonMaterial` declares a
// custom `prepass_fragment_shader`, Bevy still builds the depth-only
// `prepass_pipeline` expecting a `fragment` ENTRY POINT in this module, so simply
// `#ifdef`-ing the whole function out instead trips "no entry point was found".
// So we always provide `fragment`, and only the fragment-writing variant names
// `FragmentOutput`; the depth-only variant is a void entry that still does the
// alpha-mask discard (a no-op for the opaque props that actually reach it).

#import bevy_pbr::{
    prepass_bindings,
    prepass_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::view,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var detail_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var detail_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> params: vec4<f32>;

// Alpha-mask cutout for the grass-card tufts; a no-op when params.y == 0.
fn toon_prepass_alpha_discard(in: VertexOutput) {
#ifdef VERTEX_UVS_A
    let alpha = textureSample(detail_tex, detail_samp, in.uv).a;
    if params.y > 0.0 && alpha < params.y {
        discard;
    }
#endif
}

#ifdef PREPASS_FRAGMENT
@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    toon_prepass_alpha_discard(in);
    var out: FragmentOutput;
#ifdef NORMAL_PREPASS
    out.normal = vec4<f32>(in.world_normal * 0.5 + vec3<f32>(0.5), 1.0);
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth;
#endif
#ifdef MOTION_VECTOR_PREPASS
    let clip_position_t = view.unjittered_clip_from_world * in.world_position;
    let clip_position = clip_position_t.xy / clip_position_t.w;
    let previous_clip_position_t =
        prepass_bindings::previous_view_uniforms.clip_from_world * in.previous_world_position;
    let previous_clip_position = previous_clip_position_t.xy / previous_clip_position_t.w;
    out.motion_vector = (clip_position - previous_clip_position) * vec2<f32>(0.5, -0.5);
#endif
    return out;
}
#else
// Depth-only prepass: no `FragmentOutput`, but Bevy still wants a `fragment`
// entry point for this material's custom prepass. Void entry, discard only.
@fragment
fn fragment(in: VertexOutput) {
    toon_prepass_alpha_discard(in);
}
#endif // PREPASS_FRAGMENT
