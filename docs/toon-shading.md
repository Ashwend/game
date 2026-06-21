# Toon / cel shading (anime style)

The ore/vein nodes are the first prop in the game rendered with a **cel-shaded
(toon / anime) look** instead of the default PBR. This doc explains the style,
the shader that produces it, and how to extend it to the rest of the game.

The rest of the world (trees, terrain, buildings, players, items) is still
PBR-lit with a realism-leaning atmosphere. Mixing cel and PBR on adjacent props
reads as a mistake, so **extend by whole families, not one prop at a time** (see
[Roadmap](#roadmap)). The end state is a deliberate decision: how far toward
anime do we take the whole game.

Related: [Materials](materials.md) (PBR conventions, the standalone-Material /
Metal bind-group rules the toon material also follows), [Ore node re-model in
Items and resources](items-and-resources.md), the anime grass work (the grass
already uses cheap stylized lighting, see `src/app/scene/grass/`).

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

### Material (`OreToonMaterial`)

A standalone `Material` (NOT an `ExtendedMaterial`) so it owns its material bind
group, which keeps the texture binding alive on Metal. Bindings:

- `@texture(0)` + `@sampler(1)` `rock` — the shared hand-painted stone texture.
- `@uniform(2)` `params: Vec4` — the cel tuning, packed so it can be retuned
  **without a shader recompile** (just change the `Vec4` where the material is
  built in `assets.rs`):
  - `x` = cel band count (fewer = harder steps; ore uses `3`)
  - `y` = ambient floor (shadow-side brightness; ore uses `0.30`)
  - `z` = ink-edge strength (`0` = off; ore uses `0.8`)
  - `w` = ink-edge width exponent (smaller = wider edge; ore uses `2.2`)

One **shared** material covers all four ores because the per-mineral colour is in
the glb `COLOR_0` (grey rock body vs bright mineral chunks), so `texture *
COLOR_0` differentiates them and every ore node batches by one material.

Registered with a one-line `MaterialPlugin::<OreToonMaterial>::default()` in
`src/app.rs`, after `EmbeddedAssetsPlugin` so the embedded shader resolves.

### Fragment shader (`ore_toon.wgsl`), in plain terms

```
albedo   = textureSample(rock) * vertex_color           // painted stone * COLOR_0
wrap     = dot(N, sunDir) * 0.5 + 0.5                    // half-Lambert (soft wrap)
stepped  = floor(wrap * bands) / bands                  // quantise into cel bands
rgb      = albedo * (sun.color * stepped + ambient)     // banded sun + flat fill
edge     = pow(1 - dot(N, viewDir), width)              // silhouette mask
rgb      = mix(rgb, rgb * 0.10, edge * strength)        // darken edge -> ink outline
out      = main_pass_post_lighting_processing(rgb)      // scene fog only
```

Key conventions (all proven in `terrain.wgsl` / `grass_instanced.wgsl`):

- Bindings use `@group(#{MATERIAL_BIND_GROUP})`, **never** a literal `@group(2)`
  (which collides with Bevy 0.18's mesh bindings on Metal).
- Sun = `lights.directional_lights[0]` from `mesh_view_bindings`.
- It does its **own** lighting and only borrows `main_pass_post_lighting_processing`
  so the prop still fogs like the rest of the scene.

### Deliberate trade-off: no shadow *reception*

The hand-built cel path does not call `apply_pbr_lighting`, so it never samples
the shadow map. Ore nodes therefore **cast** shadows (the default depth/prepass
handles that) but do not **receive** them on their own surface. The grass makes
the same trade. If a future toon prop needs to catch shadows (e.g. a large flat
toon floor), use the alternate "PBR-then-posterize" path: set
`pbr_input.material.base_color = albedo`, call `apply_pbr_lighting`, then quantise
the resulting luminance into bands. Harder to control, but keeps shadow reception.

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
2. **Author/keep the meshes as they are.** Toon shading needs only
   `world_normal`, `world_position`, `uv`, `color`, `instance_index` from the
   default mesh vertex output — all already present. No new vertex attributes, no
   tangents.
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
