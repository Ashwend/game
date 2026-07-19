---
title: Art direction and look-and-feel
owns: The visual target (cel-shaded / anime over flat-shaded low-poly), the converted-vs-PBR family inventory, the palette and lighting-mood philosophy, and the art-direction trajectory.
when_to_read: Before making a new prop cel-shaded, changing the palette or lighting mood, or planning a wider art-direction shift.
sources:
  - src/app/scene/toon.rs - ToonMaterial, the one shared cel material
  - src/app/scene/sky.rs - SUN_PEAK_ILLUMINANCE, NIGHT_AMBIENT_FLOOR, compute_lighting (daylight-calibrated mood)
  - src/app/scene/assets.rs - per-family ToonMaterial params, the cel-vs-PBR material handles
  - src/app/systems/deployables.rs - DeployableMaterial::Toon vs Standard family split
  - src/app/scene/grass.rs - instanced-grass colour (per-biome tint, not uniform)
  - art/ore/prompts.json - ore identity-in-chunks rule (enforced at the reference-prompt level)
  - art/trees/build_tree.py - solid faceted canopy, up-biased normals
related:
  - docs/toon-shading.md - the cel shader mechanics + how to extend a cel family (this doc owns the why, that doc owns the how)
  - docs/rendering-materials.md - PBR StandardMaterial conventions, atmosphere/IBL, the standalone-Material Metal rules
  - docs/playbooks/art-pipeline.md - the ComfyUI Flux + Blender authoring pipeline that produces the meshes/textures
  - docs/items-and-resources.md - ore-node and tree-node definitions the art rides on
---

# Art direction and look-and-feel

> When to read this: before making a new prop cel-shaded, changing the palette or lighting mood, or planning a wider art-direction shift. Source of truth: `src/app/scene/toon.rs`, `src/app/scene/sky.rs`, `src/app/scene/assets.rs`. Canonical invariants live in CLAUDE.md.

Ashwend is mid-transition from a flat-shaded low-poly PBR survival look toward a deliberate cel-shaded (toon / anime) art direction. This doc owns the visual target, which families are converted and why, the colour and lighting philosophy, and the trajectory. It does not explain the shader (see `docs/toon-shading.md`) or the PBR triplet conventions (see `docs/rendering-materials.md`).

## Aesthetic pillars

The look is built from four ingredients, in priority order. The lighting sells it; the texture is a supporting detail.

- **Hard banded lighting.** The real lit luminance is quantised into a few flat steps (cel bands) instead of a smooth gradient. A boulder reads as 2-3 flat tones, not a continuous shade. The band count is `params.x` on every cel material and is currently `3` everywhere (`src/app/scene/assets.rs` - `ToonMaterial` params).
- **Dark ink silhouette edge.** Fragments whose normal turns away from the camera are darkened toward 10% (`assets/shaders/toon.wgsl` - the `edge` mix at strength `params.z`). This approximates a hand-drawn outline. It is a per-fragment silhouette darkening, not a true geometric outline; see [Trajectory](#trajectory).
- **Painted albedo, not photoreal.** The surface texture is a soft, low-contrast hand-painted stylized grain (ore rock, bark, needle/leaf, deployable wood/stone/fabric line-art). It only adds character; the cel bands and ink edge carry the form.
- **Identity in COLOR_0.** The per-prop colour rides on the glb `COLOR_0` vertex colours, multiplied by the detail texture (`detail * COLOR_0` in `src/app/scene/toon.rs`). Vertex-colour-only props bind a 1x1 white detail, so the multiply reduces to pure `COLOR_0`. One material per family batches the whole family; `COLOR_0` differentiates members.

The cel look is mechanically **PBR-then-posterize**: light the prop with real `apply_pbr_lighting` (the same sun + atmosphere IBL + received shadows + day/night exposure the ground gets), then divide albedo out to get the lighting *strength*, band that scalar into hard steps, and re-apply albedo so every band keeps the prop's own hue. The full shader walkthrough is in `docs/toon-shading.md`; the point here is that the cel props are lit by the *same* scene lighting as everything else, which is what keeps the converted and un-converted families coherent.

## Converted-vs-PBR family inventory

Mixing cel and PBR on adjacent props reads as a mistake, so the conversion proceeds **by whole families, not one prop at a time**. The split is deliberate and follows a natural-world-vs-man-made line: organic/natural families get the anime cel treatment, structural/man-made and character families stay real-PBR for now.

| Family | Look | Material | Evidence |
|---|---|---|---|
| Ore / vein nodes (coal, iron, sulfur, stone vein) | cel | shared `ToonMaterial` (`ore_toon_material`) | `src/app/scene/assets.rs` - `ore_toon_material` |
| Trees (pine + birch trunk + solid faceted canopy) | cel | per-species bark + foliage `ToonMaterial` | `src/app/scene/assets.rs` - `pine_bark_material` .. `birch_foliage_material` |
| Dead snags | cel | bark `ToonMaterial` over a cool-grey `COLOR_0` glb | `src/app/scene/assets.rs` - `dead_bark_material` |
| Free-standing deployables (workbench, furnace, storage boxes, torch, tool cupboard, sleeping bag) | cel | toon wood / stone / fabric `ToonMaterial` | `src/app/systems/deployables.rs` - `DeployableMaterial::Toon` |
| Harvestable hay / tall-grass tufts | cel (alpha-masked card) | `ToonMaterial` with `params.y` cutoff | `src/app/scene/assets.rs` - `hay_grass_materials` |
| GPU-instanced detail grass | cel-lit (PBR-then-posterize, not `ToonMaterial`) | custom instanced pipeline | `src/app/scene/grass/instancing.rs` + `assets/shaders/grass_instanced.wgsl` |
| Building pieces (sticks / hewn wood / stone tiers) | PBR | textured `StandardMaterial` | `src/app/scene/assets.rs` - `building_materials` |
| Doors (wood + iron) | PBR | textured `StandardMaterial` | `src/app/scene/assets.rs` - `hewn_door_material`, iron door |
| Players / remote rig | PBR | `StandardMaterial` | `src/app/scene/assets.rs` - `remote_material` |
| Held tools (stone + iron pickaxe/hatchet) | PBR | `StandardMaterial` (two-layer body/head) | `src/app/scene/assets.rs` - `held_*_body_material`, `held_*_head_material` |
| Terrain ground floor | PBR (custom `TerrainMaterial`) | biome splat-blend, daylight-lit | see `docs/rendering-materials.md` |

The deployable family routes through one of three shared cel materials: furnace uses **toon stone**, sleeping bag uses **toon fabric**, every other wooden deployable uses **toon wood** (`src/app/systems/deployables.rs` - the `DeployableMaterial` mapping). Building pieces and doors stay PBR for now (the `DeployableMaterial::Standard` branch); that is the next natural family if the conversion continues.

The instanced detail grass is in the cel-lit camp but does **not** use `ToonMaterial`: it is the project's only custom render pipeline and hand-builds its own `apply_pbr_lighting` + posterize in `grass_instanced.wgsl`. Treat it as part of the cel family for visual consistency, but edit it through the grass pipeline, not the shared material. Mechanics are in `docs/toon-shading.md` and `docs/rendering-materials.md`.

## Colour and palette philosophy

- **Vertex-colour albedos are LINEAR.** `COLOR_0` never goes through the sRGB decode that `Color::srgb` performs, so a value eyeballed as perceptual mid-grey renders ~1.5-2x brighter (chalk white in daylight). Pick `COLOR_0` physically; if converting an sRGB pick, raise it to ~2.2 power. The calibration anchor is the ground at linear `(0.027, 0.095, 0.040)`; anything that should "sit in the scene" lives within a few multiples of that. Physical-albedo conversion detail lives in `docs/rendering-materials.md`.
- **Ore identity is in the chunks, not the rock.** The rock body is one shared grey on every node; the per-mineral identity (iron rust, coal near-black, sulfur yellow, stone-vein plain knobs) rides **entirely** in the studded mineral chunks. Since the 2026-07 image-to-3D rework this rule is enforced in the reference prompts (`art/ore/prompts.json`, the shared-grey-body backbone) and carried by each type's baked albedo (`assets/textures/ore/<type>.png`); the meteorite alone gets a dark slag body. This is a user-directed decision: an earlier "tint the whole mound toward the mineral" look read as a coloured blob, and players expect chunks of ore in plain stone. Keep the rock grey.
- **Identity must read at gameplay distance.** Colour that matters belongs in bold, large surfaces (the mineral chunks, the canopy mass, the trunk), not tiny accents. Anything subtle washes out by the time the cel bands quantise it.
- **Detail grass colour is NOT uniform across biomes.** It is a per-biome tint (`biome_grass_tint` in `src/app/scene/grass.rs`): forest stays lush green, plains dries to yellow-green, rocky desaturates toward grey, ore dulls toward brown, multiplied onto the neutral blade green, plus low-frequency tonal patches and per-blade warm/cool jitter so the field reads as a painterly mass. (An earlier design tried one flat dry-green and per-biome density-only variation; the live code grades colour by biome. If a sibling doc still says "uniform grass colour," it is stale.) Density also varies by biome: bare rock/ore thin the field via `GRASS_BIOME_MAX_THIN`.

## Lighting and mood

The mood is a hand-tuned, **daylight-calibrated** atmosphere with a fixed, gameplay-fair night. Mechanics and the atmosphere/IBL setup are owned by `docs/rendering-materials.md`; the *intent* is owned here.

- **Daylight-calibrated, not physical.** The sun directional light sits at `SUN_PEAK_ILLUMINANCE = 11_000.0` lux (`src/app/scene/sky.rs`), a value chosen so the scene holds a consistent, hand-tuned brightness from dawn to dusk under the renderer's default exposure. This is deliberately **not** physical raw sunlight (~130k lux) plus a manual `Exposure`.
- **Why physical lux + auto-exposure are rejected.** Raw sunlight has too much dynamic range across the day for one fixed exposure to look good, and the usual fix, auto-exposure, would brighten the dark so the player always sees, which fights the fixed, gameplay-fair night this game wants. The atmosphere still renders and tints the sky and warms the sun toward the horizon; only the absolute scale is hand-set.
- **Fixed moonlit night.** Night brightness is a gameplay constant, not a user setting: a fixed `NIGHT_AMBIENT_FLOOR` (cool blue-grey, fades to zero by day) plus a cheated-up `MOON_PEAK_ILLUMINANCE` dim moon light so the player can navigate after dark (`src/app/scene/sky.rs` - `compute_lighting`). Daytime ambient comes from the atmosphere environment map; the night floor only fills in when the atmosphere sky goes dark.
- **The night white-blowout lesson: route cel through real PBR.** An earlier cel shader did its own cheap quantised half-Lambert lighting that ignored the sun's illuminance and view exposure, so it blew white at night and could not receive shadows. Moving the cel path (and the grass) to PBR-then-posterize fixed both: the props now dim correctly after dark and catch tree/building shadows exactly like the ground. The takeaway for any new cel surface: do not hand-roll a day-factor; light through `apply_pbr_lighting` and posterize the result.

## Per-family cel tuning

The cel tuning is data, not code: `ToonMaterial.params` is a `Vec4 = (cel band count, alpha-mask cutoff, ink-edge strength, ink-edge width exponent)`, retunable without a shader recompile (`src/app/scene/toon.rs` - `params`). The live values differ by family on purpose, so the rounded/organic props read softer than the boxy man-made ones:

| Family | `params` (bands, cutoff, edge strength, edge width exp) | Notes |
|---|---|---|
| Ore | `(3, 0, 0.8, 2.2)` | softer rounded edge |
| Tree bark | `(3, 0, 0.55, 2.6)` | gentlest edge |
| Tree foliage | `(3, 0, 0.7, 2.0)` | slightly wider so the leaf mass reads with a drawn outline |
| Dead bark | `(3, 0, 0.5, 2.6)` | |
| Hay / tall grass | `(3, 0.4, 0, 2.0)` | `params.y = 0.4` is the alpha-mask cutoff; no ink edge |
| Deployables (wood/stone/fabric) | `(3, 0, 1.0, 1.4)` | punchier, full-strength wider edge so every beveled box corner reads as a drawn line |

Values verified in `src/app/scene/assets.rs`. Note: an older consistency rule "reuse the same params as ore for every family" is aspirational, not what the live code does; the families intentionally diverge as shown.

## Trajectory

- **Extend by whole families.** Convert a family fully before moving on, so the scene never looks half-converted. The next natural candidate is the building-pieces + doors family (currently PBR), then players/held tools last (character/world consistency matters most there, and the rig is `StandardMaterial`).
- **Per-material vs a global pass.** Two real ways to push the whole game anime. (1) The **current per-material path**: convert each family's material to cel. Surgical, keeps per-material control (alpha, emissive, special cases), no render-graph work, but consistency is on us via shared `params` and a true unifying outline is hard. (2) A **global post-process cel + outline pass**: one fullscreen pass after the main pass that posterizes luminance and draws outlines from a depth+normal prepass. One place to tune, gives real geometric outlines around every object (including un-converted ones), but it is render-graph work and interacts with the existing atmosphere/fog/bloom/TAA stack. These two compose. (Aspirational: the global pass is not implemented.)
- **A true ink outline is the biggest missing signal.** The current dark edge is a per-fragment silhouette darkening; it reads as a shaded edge, not a crisp ink line. A real outline (post-process depth+normal edge detect, or per-mesh inverted-hull) is what most reads as "anime" and would unify families that have not been converted. If the art-direction shift is ever prioritized, prototype the outline pass early; it is the highest-payoff change. (Aspirational: not implemented.)

## Asset-generation toolchain

The cel meshes and textures are authored, not procedural. Most families go through a ComfyUI Flux + parametric Blender pipeline (concept image, OpenCV silhouette measurement, parametric Blender glb, soft hand-painted tileable textures); the parametric scripts live under `art/` (`art/trees/build_tree.py` and the deployable/building builders). The ore nodes instead come from the image-to-3D generation lane (`art/ore/`, reference prompts -> TRELLIS.2 mesh -> retopo + albedo rebake, the template for future family reworks). Inventory icons and tileable world textures come from the `lowpoly-game-assets` skill. The full step-by-step is in `docs/playbooks/art-pipeline.md`.

Correct names to use when extending the cel family (the old `OreToonMaterial` / `ore_toon.rs` / `ore_toon.wgsl` names are gone):

- Material struct: `ToonMaterial` in `src/app/scene/toon.rs`.
- Shader: `assets/shaders/toon.wgsl` (loaded as `embedded://shaders/toon.wgsl`).
- Registered once as `MaterialPlugin::<ToonMaterial>::default()` in `src/app.rs`.

## Related docs

- `docs/toon-shading.md` - the cel shader mechanics (PBR-then-posterize, the bindings, the `#ifdef` attribute guards) and the step-by-step for adding a prop to the cel family.
- `docs/rendering-materials.md` - PBR `StandardMaterial` conventions (the matte reflectance/roughness/metallic triplets), the atmosphere + IBL lighting setup, the linear-albedo math, and the standalone-Material / `@group(#{MATERIAL_BIND_GROUP})` Metal rules the cel material also follows.
- `docs/playbooks/art-pipeline.md` - authoring a new model or icon (Blender MCP, OpenCV silhouette measurement, glb export gotchas, the ComfyUI Flux texture path).
- `docs/items-and-resources.md` - the ore-node and tree-node definitions and gather rules the art rides on.
