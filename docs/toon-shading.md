---
title: Cel shader mechanics and extending a cel family
owns: the ToonMaterial cel shader (PBR-then-posterize), its bindings/params, and the procedure for making a new prop family cel-shaded
when_to_read: Before editing the cel shader, adding a prop to the ToonMaterial family, or retuning band/edge hardness.
sources:
  - src/app/scene/toon.rs - ToonMaterial (bindings, alpha_mode)
  - assets/shaders/toon.wgsl - cel fragment shader (PBR-then-posterize, triplanar, consts)
  - src/app/scene/assets.rs - setup_scene (per-family ToonMaterial params)
  - src/app/scene/mesh/builder.rs - build_hay_tuft_mesh (double-sided alpha card)
  - src/app/systems/node_death.rs - apply_fade_out (felling fade uniform)
  - src/app/systems/items/resource_nodes/spawn.rs - ResourceNodeMaterial enum
  - src/app/systems/deployables.rs - deployable family to material map
  - src/app.rs - MaterialPlugin::<ToonMaterial> registration
related:
  - docs/art-direction.md - the look-and-feel vision, what-is-cel-vs-PBR inventory, roadmap and palette philosophy
  - docs/rendering-materials.md - StandardMaterial PBR conventions, the standalone-Material/Metal bind-group rules this shader also follows, lighting
  - docs/playbooks/art-pipeline.md - authoring a glb/texture for a new cel prop (Blender + ComfyUI pipeline)
  - docs/items-and-resources.md - resource-node spawn sites that attach ToonMaterial
---

# Cel shader mechanics and extending a cel family

> When to read this: before editing the cel shader, adding a prop to the `ToonMaterial` family, or retuning band/edge hardness. Source of truth: `src/app/scene/toon.rs`, `assets/shaders/toon.wgsl`, and the per-family `params` in `src/app/scene/assets.rs`. Canonical invariants live in CLAUDE.md.

This doc owns the cel shader **mechanics**: how `ToonMaterial` PBR-then-posterizes a prop, its bindings and `params` packing, the real per-family values, and the steps to make a new prop family cel-shaded. For the art-direction *vision* (how far toward anime the game goes, the cel-vs-PBR family inventory, palette philosophy, the roadmap), read [art-direction.md](art-direction.md).

The material is `ToonMaterial` in `src/app/scene/toon.rs`, shader `assets/shaders/toon.wgsl` (loaded as `embedded://shaders/toon.wgsl`). There is no `OreToonMaterial`, no `ore_toon.rs`, and no `ore_toon.wgsl`; those names were retired when the ore-only material was generalised. If a doc or comment still says `OreToonMaterial`, it is stale.

## What ships cel-shaded

One shared `ToonMaterial` asset type (registered once) covers every cel prop:

- Ore/vein nodes (all four), via `ResourceVisualAssets::ore_toon_material` (`src/app/scene/assets.rs` - `setup_scene`).
- Trees: pine + birch bark trunks, the **solid faceted** canopies, and the dead snags.
- The harvestable hay/tall-grass cards.
- Every free-standing deployable: workbench, furnace, storage boxes, torch, tool cupboard, sleeping bag (`src/app/systems/deployables.rs`).

The GPU-instanced detail grass (`assets/shaders/grass_instanced.wgsl`) is a *separate* material but shares the same PBR-then-posterize idiom and the same band/fill constants (see [Cross-shader constant lockstep](#cross-shader-constant-lockstep)).

Still `StandardMaterial` (not cel): building pieces, doors (wood + iron), players/remote rig, the four held tools, LOD trees, placement ghosts, impact debris. The cel-vs-PBR boundary is deliberate and load-bearing; the full inventory and the rule "convert by whole families, never one adjacent prop" live in [art-direction.md](art-direction.md).

## PBR-then-posterize, precisely

The prop is lit by the **real engine PBR** (`apply_pbr_lighting`), then the lit luminance is quantised into hard cel bands. Going through real PBR is the whole point: the prop inherits the scene's sun + atmosphere IBL + **received shadows** + day/night exposure, so it dims correctly after dark and catches tree/building shadows exactly like the ground. The earlier hand-rolled half-Lambert ignored illuminance + view exposure and blew white at night; that bug is why the family routes through PBR. Do not re-add a hand-rolled day-factor uniform.

The fragment path (`assets/shaders/toon.wgsl` - `fragment`):

```
albedo  = detail_sample * COLOR_0                              // painted detail texture * vertex colour
pbr     = hand-built PbrInput { base_color=albedo, matte }     // roughness 0.95, reflectance 0, metallic 0
lit     = apply_pbr_lighting(pbr)                              // real sun + IBL + received shadows + exposure
shade   = clamp(luminance(lit) / luminance(albedo), 0, 0.999) // lighting STRENGTH, hue divided out
banded  = clamp(floor(shade*bands)/bands * TOON_LIT_GAIN, 0,1) // quantise strength into hard cel bands
shade_q = max(banded, shade * TOON_SHADOW_FILL)               // dark end tracks real shade, no flat cliff
rgb     = albedo * shade_q                                     // re-apply albedo -> keeps the prop's own hue
edge    = pow(1 - clamp(dot(N, V), 0, 1), max(w, 0.5))         // silhouette mask (w = params.w)
rgb     = mix(rgb, rgb * 0.10, clamp(edge * z, 0, 1))          // darken edge -> ink outline (z = params.z)
rgb     = max(mix(grey(rgb), rgb, TOON_SATURATION), 0)         // saturation lift (value unchanged)
out     = main_pass_post_lighting_processing(rgb, lit.a*fade)  // scene fog, per-instance fade in alpha
```

Two subtleties that took iteration:

- **Band the strength, not the lit colour.** Posterising the lit colour directly tinted every band by the lighting; on a shadow side that lighting is the desaturated sky ambient, so the prop washed out to flat grey. Dividing albedo out (`shade`), banding the scalar, then re-multiplying `albedo` keeps the prop's hue in every band.
- **`TOON_SHADOW_FILL`, not a flat floor.** The darkest region follows `shade * TOON_SHADOW_FILL` rather than a constant, so a *daytime* shadow side (ambient-lit, moderate shade) stays dim-but-present while *night* (very low shade) goes genuinely dark. No flat near-black cliff on a side-lit surface.

The `PbrInput` is hand-built (mirroring `terrain.wgsl`): matte (`perceptual_roughness = 0.95`, `reflectance = vec3(0)`, `metallic = 0`) so no glossy Fresnel streak fights the bands, with the mesh's shadow-receiver flag copied from `mesh[in.instance_index].flags`. The shader deliberately does **not** import `pbr_fragment` (it would pull a second `StandardMaterial` binding set into the material group).

### Shadows

Solid cel props (ore, trees, deployables) both **cast** (default depth/prepass) and **receive** (the PBR path samples the shadow map). The instanced grass shares the same posterize fragment and also **receives** but does **not cast** (no shadow-pass pipeline for the instanced draw; thousands of thin-blade shadows would read as noise and cost a re-render per cascade). That split is owned by the grass pipeline, not this material.

## Shader tuning constants (WGSL `const`)

These live in `assets/shaders/toon.wgsl`, not in `params`, so changing them retunes **all** cel families at once. Edit here to shift cross-family band hardness or saturation:

| Const | Value | Role |
|---|---|---|
| `TOON_SATURATION` | `1.25` | Chroma lift applied after banding so the banded result keeps the bright anime feel. Value (luma) is unchanged, so it does not blow highlights. `1.0` = off. |
| `TOON_LIT_GAIN` | `1.5` | Scales the bands so the brightest reaches ~full albedo by day. |
| `TOON_SHADOW_FILL` | `0.6` | Floor multiplier; the dark end follows `shade * TOON_SHADOW_FILL` instead of a flat constant. |

`bands` is `max(params.x, 2.0)`; the ink-edge dark target is the hardcoded `* 0.10`.

## ToonMaterial bindings

`src/app/scene/toon.rs` - `ToonMaterial`. Bindings map 1:1 with the shader's `@group(#{MATERIAL_BIND_GROUP})` block:

| Binding | Field | Role |
|---|---|---|
| `@texture(0)` + `@sampler(1)` | `detail: Handle<Image>` | Per-family detail texture (ore rock grain, deployable wood/stone/fabric, tree bark/foliage, tall-grass cards). Every current prop binds a real detail texture; none is vertex-colour-only. The `detail * COLOR_0` math supports binding a 1x1 white image to reduce to pure `COLOR_0`, but nothing in the scene uses that fallback today. The sampler is taken from this image's sampler. |
| `@uniform(2)` | `params: Vec4` | Cel tuning (see below). Retunable without a shader recompile. |
| `@uniform(3)` | `tex_scale: f32` | Texture tiles/metre for the triplanar path used by UV-less meshes. Dead: every shipping family carries UVs, so this is never read (see [Triplanar path](#triplanar-path-tex_scale-is-dead)). |
| `@uniform(4)` | `fade: f32` | Per-instance opacity. `1.0` for every static prop; only the tree-felling dissolve drives it below `1.0`. |
| `@texture(6)` + `@sampler(7)` | `emissive_tex: Handle<Image>` | Night-glow vein mask (bright = extra glow). Every prop binds the shared 1x1 white "no glow" image (`toon_no_glow_tex`), so with a zero `emissive` tint the term is inert. |
| `@uniform(8)` | `emissive: Vec4` | Self-illumination. `rgb` = HDR-bright glow colour ADDED on top of the cel-lit surface (after the day/night-exposed cel term); `a >= 0.5` gates the glow by COLOR_0 vertex alpha so one mesh could mix glowing and non-glowing geometry. `Vec4::ZERO` = no emission, which is what EVERY material ships today. See [Emissive term (currently unused)](#emissive-term-currently-unused). |

### `params: Vec4` packing

- `x` = cel band count (fewer = harder steps; everything ships `3`).
- `y` = alpha-mask cutoff. `0` = opaque (every solid prop). `> 0` turns the material into an alpha-masked card: the shader discards texture alpha below the cutoff, and `alpha_mode()` returns `Mask(params.y)`. Used only by the hay tuft (`0.4`).
- `z` = ink-edge strength (`0` = off).
- `w` = ink-edge width exponent (smaller = wider edge; clamped to `>= 0.5` in the shader).

### `alpha_mode()` logic

`src/app/scene/toon.rs` - `ToonMaterial::alpha_mode`:

- `params.y > 0.0` -> `AlphaMode::Mask(params.y)` (hard cutout card, draws in the opaque/alpha-mask pass, depth-correct, no sort).
- else `fade < 1.0` -> `AlphaMode::Blend` (the felling dissolve flips the *cloned* material into the transparent pass).
- else -> `AlphaMode::Opaque` (the common case; cel props draw in the cheap opaque pass and depth-occlude the transparent detail grass correctly).

## Emissive term (currently unused)

No material self-illuminates today: the world has no magic, and the one former user (the meteorite node's glowing crystal spikes) was retired when the node became a plain scorched slag boulder with alloy nuggets on the shared ore material. The shader path remains for future non-magical light sources (e.g. a lit window, coals in a brazier):

- **One additive term, after the cel pass.** The shader adds `emissive.rgb * glow_amount * glow_gate` to the posterised `rgb` just before fog/tonemapping, so a glow stays visible at night without folding into the lighting.
- **Vertex-alpha gate, not a separate mesh.** `emissive.a >= 0.5` gates the glow by the mesh's COLOR_0 vertex alpha, so a single glb can mix glowing and non-glowing geometry. Every current build writes alpha 1.0, which is inert because every tint is zero.
- **The `emissive_tex` mask BOOSTS, it does not gate**: the shader samples it as `glow_amount = 0.55 + 0.45 * mask`.
- **Inert everywhere.** All `ToonMaterial` sites bind the shared 1x1 white `toon_no_glow_tex` and `Vec4::ZERO`, so the `if emissive.r/g/b > 0` branch is skipped and output is unchanged by the term's existence.

## Triplanar path (`tex_scale` is dead)

`tex_scale` is **dead**. Every shipping `ToonMaterial` (ore, trees, hay, and deployables) carries UVs and takes the `#ifdef VERTEX_UVS_A` branch, sampling `detail` directly; `tex_scale` is never read. The deployable glbs used to be UV-less and triplanar-projected, but `art/deployables/build_deployables.py` now bakes box-projected UVs into each model, so they sample `detail` like the rest. The deployable wood/stone/fabric materials still set `tex_scale: 1.5` (`src/app/scene/assets.rs` - `setup_scene`), but it is unused, just like the `tex_scale: 1.0` on the other families. The triplanar `#else` path still exists in the shader as the UV-less fallback, but no current material exercises it.

## Per-family params (real values that ship)

From `src/app/scene/assets.rs` - `setup_scene` (and the `hay_tall_grass_material` helper in `src/app/scene/materials.rs`). The values intentionally **differ** by family: rounded/organic props (ore, trees) run a softer, narrower ink edge; the boxy deployables run a punchier full-strength wider edge so every beveled corner reads as a drawn outline.

| Family | `params` (x,y,z,w) | `tex_scale` | Shape rationale |
|---|---|---|---|
| Ore/vein nodes | `(3, 0, 0.8, 2.2)` | `1.0` | Reference impl; rounded boulder, medium-strength edge, UV-mapped. |
| Tree bark (pine + birch) | `(3, 0, 0.55, 2.6)` | `1.0` | Softer, narrower edge for organic trunks. |
| Tree foliage (pine + birch) | `(3, 0, 0.7, 2.0)` | `1.0` | Slightly wider edge so the leafy mass reads with a drawn silhouette. |
| Dead bark (snags) | `(3, 0, 0.5, 2.6)` | `1.0` | Softest edge; same pine-bark detail, cool-grey `COLOR_0` reads weathered. |
| Hay / tall-grass card | `(3, 0.4, 0, 2.0)` | `1.0` | Alpha-mask cutout (`y=0.4`), no ink edge (`z=0`), card carries its own UVs. |
| Deployables (wood/stone/fabric) | `(3, 0, 1.0, 1.4)` | `1.5` | Punchier full-strength (`z=1.0`) wide (`w=1.4`) edge for boxy faces; box-projected UVs, so `tex_scale=1.5` is set but unused. |

Deployable family -> material map (`src/app/systems/deployables.rs`): furnace -> `toon_stone_material`; workbench / storage box / tool cupboard / torch -> `toon_wood_material`; sleeping bag -> `toon_fabric_material`. Buildings and doors stay `StandardMaterial`.

When retuning, change the relevant `params` `Vec4` (per family) or the WGSL consts (all families). Reuse an existing family's `params` unless you have an art reason to differ; matching banding hardness across the scene is what keeps the look coherent.

## Vertex-attribute guards

The shader reads `world_normal`, `world_position`, `instance_index` (always present) plus *optionally* `uv` (TEXCOORD_0) and `color` (COLOR_0). Accessing `in.uv` / `in.color` on a mesh that lacks the attribute **fails to compile that pipeline**, and the prop renders **invisible while still casting a shadow** (a shadow with no body is the tell). The shader therefore guards both behind `#ifdef VERTEX_UVS_A` / `#ifdef VERTEX_COLORS` and falls back: missing UV -> triplanar; missing colour -> white. Keep those guards when extending; any new cel mesh just needs `COLOR_0` (or accept the white fallback).

## Felling fade (binding 4)

Opaque cel props cannot fade via base-color alpha (no base-color uniform), so the tree-felling dissolve uses the per-instance `fade` uniform. `src/app/systems/node_death.rs`:

- On felling, the trunk and canopy `ToonMaterial` handles are **cloned** (`materials.add(source.clone())`), so driving fade on the clone never drags every other tree's shared material.
- The clone starts at `fade == 1.0` (opaque) so the still-upright trunk draws in the opaque pass and depth-occludes the detail grass. Forcing `Blend` up front let grass punch through.
- `apply_fade_out` ramps `material.fade` from `1.0` to `0.0` after a landed hold. Crossing below `1.0` flips that clone to `AlphaMode::Blend` automatically (via `alpha_mode()`), so the fade actually blends.

Setting `fade < 1.0` on a **shared** material would drag every instance into the transparent pass. Always clone first.

## Metal / bind-group rules

These are shared with `terrain.wgsl` and `grass_instanced.wgsl`; the reasoning lives in [rendering-materials.md](rendering-materials.md), summarised here as it directly constrains `ToonMaterial`:

- **Standalone `Material`, not `ExtendedMaterial`.** `ExtendedMaterial`'s bind-group merge with bindless `StandardMaterial` drops the extension bindings on Metal at pipeline creation. A standalone material owns its bind group, keeping the texture binding alive.
- **`@group(#{MATERIAL_BIND_GROUP})`, never a literal `@group(2)`.** In Bevy 0.18 the mesh array lives at literal group 2 and the material group is group 3 via the shaderdef. Hardcoding `2` gives a runtime "Bindings conflict" naga error.
- **Double-sided via baked back-faces, not `cull_mode = None`.** To make a cel *card* double-sided (hay), emit reversed-winding back-faces in the mesh (`build_hay_tuft_mesh` doubles each quad's winding, pins vertex alpha to `1.0` so the cutout is texture-only). A per-material `cull_mode = None` would also turn the solid ore/tree props double-sided. Solid single-sided meshes (tree canopies) instead run `recalc_face_normals` in the build script for outward winding.
- **Registered exactly once.** `MaterialPlugin::<ToonMaterial>::default()` in `src/app.rs` (one shared plugin/asset type for all families). It is client-only; the dedicated server has no render app. Any scene test that runs `setup_scene` must `app.init_asset::<ToonMaterial>()` (the bare test app in `src/app/scene.rs` does this).

## Extending to a new prop family

The cel-vs-PBR conversion *decision* (whole families only, what is deliberately still PBR, the roadmap, and the open per-material-vs-global-pass strategy) belongs to [art-direction.md](art-direction.md). The **mechanical** steps:

1. **Author the mesh + texture.** Give the glb `COLOR_0` vertex colours (linear albedos; see the colour rules in [rendering-materials.md](rendering-materials.md)). Supply a soft, low-contrast detail texture, or a 1x1 white image for vertex-colour-only props. The full Blender + ComfyUI authoring pipeline is in [playbooks/art-pipeline.md](playbooks/art-pipeline.md). Decode the PNG with `build_mip_chain` (`src/app/scene/terrain.rs`) + the `tree_texture_sampler` (repeat + aniso), the shared loader for every embedded cel texture, because Bevy 0.18 builds no mips for loaded PNGs.
2. **Pick `params`.** Reuse an existing family's row from the table above unless you have an art reason to differ. Alpha card? Set `params.y` to the cutoff and bake reversed-winding back-faces (see hay). Triplanar (no UVs)? Set `tex_scale` like the deployables.
3. **Attach the material at the spawn site.** Swap `MeshMaterial3d<StandardMaterial>` for `MeshMaterial3d<ToonMaterial>`. The clean pattern when only *some* models switch is a small enum plus a centralised helper, so every spawn site stays in sync: `ResourceNodeMaterial { Standard, Toon }` + `insert_resource_node_material` (`src/app/systems/items/resource_nodes/spawn.rs`); deployables use `DeployableMaterial { Standard, Toon }` (`src/app/systems/deployables.rs`).
4. **No new registration.** Reuse the single shared `MaterialPlugin::<ToonMaterial>`. Add `app.init_asset::<ToonMaterial>()` to any test that runs `setup_scene`.
5. **Keep the guards.** Do not unconditionally read `in.uv` / `in.color` (see [Vertex-attribute guards](#vertex-attribute-guards)).

### Validation

Build with the headless harness ([headless-agent-testing.md](headless-agent-testing.md)) and check in **daylight** (`/time 12`; the world spawns at night, where banding is hard to read). Spawn props within interaction reach so admin cleanup works. The tell for the missing-attribute bug is a shadow with no visible body. Verify at range too: the material runs `main_pass_post_lighting_processing` so distant cel props fade with the scene fog.

## Cross-shader constant lockstep

Three shaders share the PBR-then-posterize idiom: `toon.wgsl`, `grass_instanced.wgsl`, and (for the hand-built PbrInput pattern) `terrain.wgsl`. The grass shader duplicates the band constants as `GRASS_LIT_GAIN = 1.5` and `GRASS_SHADOW_FILL = 0.6`, matching `TOON_LIT_GAIN` / `TOON_SHADOW_FILL`. They are kept in lockstep on purpose so the families band consistently; if you retune one set, retune the other.

## Related docs

- [art-direction.md](art-direction.md) - the cel look-and-feel vision, the cel-vs-PBR family inventory, the roadmap, and palette philosophy.
- [rendering-materials.md](rendering-materials.md) - `StandardMaterial` PBR conventions, the standalone-Material / Metal bind-group rules this shader also follows, atmosphere/IBL lighting, linear `COLOR_0` albedos.
- [playbooks/art-pipeline.md](playbooks/art-pipeline.md) - authoring a glb or texture for a new cel prop (Blender + ComfyUI + OpenCV).
- [items-and-resources.md](items-and-resources.md) - resource-node spawn sites that attach `ToonMaterial`.
