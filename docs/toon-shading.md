# Toon / cel shading (anime style)

A **cel-shaded (toon / anime) look** rendered as **PBR-then-posterize**: light the
prop with the real engine PBR, then quantise the lit strength into a few hard
bands. This doc explains the style, the shader that produces it, and how to extend
it to the rest of the game.

**Current coverage:** the shared `ToonMaterial` cel-shades the **ore/vein nodes**
and **all free-standing deployables** (workbench, furnace, storage, torch, tool
cupboard, sleeping bag); the **instanced grass** (`grass_instanced.wgsl`) uses the
same PBR-then-posterize lighting. The biome **ground textures** were repainted in
the toony style but the terrain itself is still smooth PBR (cel-banding the ground
is the riskiest step, see [Roadmap](#roadmap)). Trees, buildings, players, items,
and the doors remain PBR.

> Historical note: the ore shader originally did its *own* cheap quantised
> half-Lambert lighting (no `apply_pbr_lighting`), which ignored the sun's
> illuminance/exposure and so blew white at night and couldn't receive shadows.
> It (and the grass) were moved to the PBR-then-posterize path below, which fixes
> both. Some sections further down still describe the family-by-family rollout as
> a plan; most of it is now done.

Mixing cel and PBR on adjacent props reads as a mistake, so **extend by whole
families, not one prop at a time**. The end state is a deliberate decision: how
far toward anime do we take the whole game.

Related: [Materials](materials.md) (PBR conventions, the standalone-Material /
Metal bind-group rules the toon material also follows), [Ore node re-model in
Items and resources](items-and-resources.md).

## What the style is

- **Hard banded lighting.** The sun's diffuse is quantised into a few flat steps
  (cel bands) instead of a smooth gradient. A boulder reads as 2-3 flat tones,
  not a continuous shade.
- **Flat ambient fill.** The shadow side never crushes to black; a flat ambient
  floor keeps it readable and gives the "filled" look of cel art.
- **Dark ink edge.** Fragments whose normal turns away from the camera are
  darkened, approximating a hand-drawn silhouette outline. Cheap and per-fragment
  (not a real geometric outline, see [True outlines](#true-outlines-vs-the-fragment-edge)).
- **Painted albedo, not photoreal.** The surface texture is a hand-painted
  stylized stone (soft, low-contrast), and the colour identity rides on the mesh
  `COLOR_0` vertex colours. The cel lighting is what sells the look; the texture
  just adds a little character.

## The shader

Two files, mirroring the `TerrainMaterial` pattern in [Materials](materials.md):

- `src/app/scene/ore_toon.rs` — `OreToonMaterial`, a standalone Bevy `Material`.
- `assets/shaders/ore_toon.wgsl` — the cel fragment shader.

### Material (`ToonMaterial`)

A standalone `Material` (NOT an `ExtendedMaterial`) so it owns its material bind
group, which keeps the texture binding alive on Metal. One **shared**
`ToonMaterial` covers the ore nodes AND every free-standing deployable (workbench,
furnace, storage, torch, tool cupboard, sleeping bag). Bindings:

- `@texture(0)` + `@sampler(1)` `detail` — the per-family detail texture (ore rock
  grain; deployable wood / stone / fabric line-art; a 1×1 white image reduces it
  to pure `COLOR_0`).
- `@uniform(2)` `params: Vec4` — the cel tuning, packed so it can be retuned
  **without a shader recompile** (just change the `Vec4` in `assets.rs`):
  - `x` = cel band count (fewer = harder steps; ore + deployables use `3`)
  - `y` = unused (was the flat ambient floor; PBR now supplies ambient via the
    atmosphere IBL)
  - `z` = ink-edge strength (`0` = off)
  - `w` = ink-edge width exponent (smaller = wider edge)
- `@uniform(3)` `tex_scale` — triplanar tiles/m for the no-UV fallback (now dead:
  every prop carries baked UVs).

The per-family / per-mineral colour rides on the glb `COLOR_0` (grey rock vs
bright mineral chunks; wood brown; fabric green), so `texture * COLOR_0`
differentiates them and a family batches by one material. Registered with
`MaterialPlugin::<ToonMaterial>::default()` in `src/app.rs`.

### Fragment shader (`toon.wgsl`), in plain terms: PBR-then-posterize

The prop is lit by **real PBR** (`apply_pbr_lighting`), then the result is
quantised into cel bands. Going through PBR is what makes it inherit the scene's
actual sun + atmosphere IBL + **received shadows** + day/night exposure (so it
dims correctly after dark and catches tree/building shadows, exactly like the
ground); the posterise keeps the anime read.

```
albedo  = textureSample(detail) * vertex_color           // painted detail * COLOR_0
lit     = apply_pbr_lighting(pbr{base_color=albedo, matte})  // real sun+IBL+shadows+exposure
shade   = lit_luminance / albedo_luminance               // lighting STRENGTH (hue divided out)
band    = clamp(floor(shade*bands)/bands * LIT_GAIN, 0,1) // quantise into hard cel bands
shade_q = max(band, shade * SHADOW_FILL)                 // dark end tracks the real shade
rgb     = albedo * shade_q                               // re-apply albedo -> keeps the prop's hue
edge    = pow(1 - dot(N, viewDir), width)                // silhouette mask
rgb     = mix(rgb, rgb * 0.10, edge * strength)          // darken edge -> ink outline
out     = main_pass_post_lighting_processing(rgb)        // scene fog
```

Two subtleties that took iteration:

- **Band the strength, not the lit colour.** Posterising the lit colour directly
  tinted every band by the lighting; on a shadow side that lighting is the
  desaturated sky-ambient, so the prop washed out to a flat grey. Dividing albedo
  out, banding the scalar strength, and re-applying albedo keeps the prop's own
  hue in every band.
- **`SHADOW_FILL`, not a flat floor.** The darkest region follows `shade *
  SHADOW_FILL` rather than a constant, so a *daytime* shadow side (ambient-lit,
  moderate shade) stays dim-but-present while *night* (very low shade) goes
  genuinely dark, no flat near-black cliff on a side-lit surface.

Key conventions (shared with `terrain.wgsl` / `grass_instanced.wgsl`):

- Bindings use `@group(#{MATERIAL_BIND_GROUP})`, **never** a literal `@group(2)`
  (which collides with Bevy 0.18's mesh bindings on Metal).
- The `PbrInput` is hand-built (like `terrain.wgsl`), matte (roughness 1, zero
  reflectance) so no glossy streak fights the bands, with the shadow-receiver flag
  set. We deliberately don't import `pbr_fragment` (it would pull a second
  StandardMaterial binding set into the material group).

### Shadows

Ore + deployables both **cast** (default depth/prepass) and **receive** (the PBR
path samples the shadow map). The instanced grass (`grass_instanced.wgsl`) shares
the same PBR-then-posterize fragment and also **receives**, but does **not cast**:
there's no shadow-pass pipeline for the instanced draw, deliberate, since
thousands of thin-blade shadows read as noise and cost a re-render per cascade.

## Extending to a new prop family

The pattern is small and repeatable. Two ways to structure it; pick per the
[Architecture](#architecture-per-material-vs-a-global-pass) section.

### Per-material path (what ore does)

1. **Generalise the material.** Rather than copy `OreToonMaterial` per family,
   promote it to a shared `ToonMaterial` with the fields each family needs:
   - `base: Handle<Image>` + sampler (the family's texture; pass a 1x1 white
     image for vertex-colour-only props).
   - `params: Vec4` (the cel tuning — share one tuning across families for a
     consistent look, or vary per family intentionally).
   - For alpha-masked families (foliage, grass cards, hay), add an
     `alpha_cutoff` and `discard` in the shader, and override
     `Material::alpha_mode()` + `specialize()` `cull_mode = None` (mirror the
     hay/foliage `StandardMaterial` settings).
2. **Author/keep the meshes as they are.** Toon shading reads `world_normal`,
   `world_position`, `instance_index`, and *optionally* `uv` + `color` from the
   default mesh vertex output. No new vertex attributes, no tangents. **Gotcha:**
   `uv` (TEXCOORD_0) and `color` (COLOR_0) are per-mesh optional, and accessing
   `in.uv`/`in.color` in WGSL on a mesh that lacks the attribute **fails to
   compile that pipeline**, so the mesh renders *invisible while still casting a
   shadow* (a shadow with no body is the tell). The ore glbs have both; most
   deployable glbs have COLOR_0 but **no UVs**. The shader therefore guards both
   accesses behind `#ifdef VERTEX_UVS_A` / `#ifdef VERTEX_COLORS` and falls back
   to white, so any mix of textured / vertex-colour-only props just works. Keep
   those guards when extending.
3. **Attach the material.** Swap `MeshMaterial3d<StandardMaterial>` for
   `MeshMaterial3d<ToonMaterial>` at the spawn site. The ore code shows the clean
   way to do this when only *some* models switch: a small enum
   (`ResourceNodeMaterial { Standard, Toon }`) + an `insert_*_material` helper so
   each spawn site stays in sync (`src/app/systems/items/resource_nodes/spawn.rs`).
4. **Register** one more `MaterialPlugin::<ToonMaterial>` (or reuse the shared
   one) and **add the asset to any test that runs the setup system**
   (`app.init_asset::<ToonMaterial>()`, see the scene-test helper).

### Roadmap

Ordered by effort and visual payoff. Do a family fully before moving on, so the
scene never looks half-converted.

| Family | Effort | Notes / gotchas |
|---|---|---|
| **Ore/vein nodes** | done | the reference implementation |
| **Deployables** (workbench, furnace, boxes, tool cupboard, torch, doors) | low | opaque vertex-coloured glbs, exactly like ore; the furnace's ember glow should stay emissive (add an emissive term or keep that bit PBR) |
| **Crude clutter + surface stone + branch piles + building pieces** | low | opaque, vertex-coloured or simple-textured; same path |
| **Tree bark / dead snags** | medium | opaque trunks are easy; but trees are the most numerous prop, so check the cel cost at forest scale and that the LOD stand-ins match |
| **Tree foliage / hay** | medium-high | alpha-masked + double-sided; needs the `discard` + `cull_mode = None` toon variant, and the up-biased foliage normals already in the glbs will read nicely under cel |
| **Grass** | medium | already a custom stylized pipeline (`grass_instanced.wgsl`); add cel banding to its existing hand-built lighting rather than a new material |
| **Players / held items** | medium | rigged + animated; the rig and the four tool glbs use `StandardMaterial`. Visual consistency with the world matters most here |
| **Terrain ground** | high | huge surface, already a custom `TerrainMaterial`; cel-banding the ground is the biggest tonal change and the easiest to get wrong (banding artifacts on gradients, shadow reception matters here). Do this last and carefully |

### Architecture: per-material vs a global pass

Two real options for "make the game anime":

1. **Per-material toon (incremental, current path).** Convert each family's
   material to cel shading. Pros: surgical, keeps per-material control (alpha,
   emissive, special cases), no new render-graph work. Cons: every family is a
   small task; consistency is on us to maintain via shared `params`; a true
   unifying outline is hard.
2. **Global post-process cel + outline pass.** Add a fullscreen pass after the
   main pass: posterise luminance and draw outlines from depth+normal edge
   detection. Pros: one place to tune; gives **real geometric outlines** around
   every object for a much stronger anime read; converts the whole scene at once.
   Cons: a render-graph node is more upfront work; interacts with the existing
   atmosphere/fog/bloom/TAA stack (order matters); less per-object control.

**Recommendation:** do the cheap per-material families first (deployables, crude,
bark) to validate the look at scene scale, **and** prototype the global outline
pass early, because a real outline is what most reads as "anime" and it unifies
families you haven't converted yet. The two compose: per-material cel for the
flat banding, global pass for outlines + a final posterise.

### True outlines vs the fragment edge

The ore shader's dark edge is a per-fragment silhouette darkening (cheap, no extra
geometry). It reads as a shaded edge, not a crisp ink line. For real outlines:

- **Inverted-hull**: render each mesh a second time, scaled along normals, back
  faces only, in black. Per-mesh, crisp, but doubles draw calls and needs clean
  normals.
- **Post-process edge detect** (preferred for a whole-game look): one fullscreen
  pass reading the depth + normal prepass, drawing lines where they discontinue.
  Outlines every object including un-converted ones. This is the global-pass
  option above.

## Consistency checklist when converting a family

- Reuse the **same `params`** (band count, ambient, edge) as ore unless you have a
  reason to differ, so the banding hardness matches across the scene.
- Keep emissive/glowing bits (furnace embers, torch flame, tip-glow) — fold a
  small additive emissive into the toon shader rather than dropping it.
- Respect alpha: foliage/grass/hay need `discard` + double-sided.
- Watch the **atmosphere/fog** interplay: the toon material still runs
  `main_pass_post_lighting_processing` for fog, so distant toon props fade like
  the rest of the scene. Verify at range.
- Re-validate **day/night**: the flat ambient floor means toon props don't go
  fully black at night (intended), but check they don't look flat-lit under a low
  sun. The ore `params.y = 0.30` is tuned for this; reuse it.
- Run the headless harness (see [Multiplayer testing](multiplayer-testing.md))
  in **daylight** (`/time 12`; the world spawns at night) and spawn props within
  interaction reach so admin cleanup works.
