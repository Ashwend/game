---
title: PBR material conventions and lighting
owns: The PBR `StandardMaterial` triplet conventions, the atmosphere/IBL lighting model, the flat-surface normal-jitter recipe, the shared texture loader, and `TerrainMaterial`.
when_to_read: Before adding a new `StandardMaterial` or tuning reflectance/roughness/metallic on a PBR prop, or changing the atmosphere/IBL lighting.
sources:
  - src/app/scene/assets.rs - ATMOSPHERE_AMBIENT_INTENSITY, ATMOSPHERE_ENV_MAP_SIZE, camera/IBL spawn, StandardMaterial handles
  - src/app/scene/materials.rs - tree_texture_sampler
  - src/app/scene/world.rs - flat_ground_material, build_ground_mesh, stone_material
  - src/app/scene/terrain.rs - TerrainMaterial, build_mip_chain
  - src/app/scene/sky.rs - SUN_PEAK_ILLUMINANCE, MOON_PEAK_ILLUMINANCE, NIGHT_AMBIENT_FLOOR
  - src/app/scene/toon.rs - ToonMaterial (the cel path; details in toon-shading.md)
related:
  - docs/toon-shading.md - the cel ToonMaterial shader mechanics; everything cel-shaded lives there
  - docs/art-direction.md - the visual target and the cel-vs-PBR family boundary
  - docs/worlds-and-saves.md - biome classification that drives the TerrainMaterial weight raster
  - docs/profiling.md - why the env map is 64px and other render-cost levers
---

# PBR material conventions and lighting

> When to read this: before adding a new `StandardMaterial`, tuning reflectance/roughness/metallic on a PBR prop, or changing the atmosphere/IBL lighting. Source of truth: `src/app/scene/assets.rs`, `src/app/scene/world.rs`, `src/app/scene/terrain.rs`, `src/app/scene/sky.rs`. Canonical invariants live in CLAUDE.md.

This doc owns the **PBR** half of the client's look: matte `StandardMaterial` conventions, the atmosphere/IBL lighting model, the flat-surface normal-jitter recipe, the shared texture loader, and the biome-splat `TerrainMaterial`. The **cel** half (`ToonMaterial`, the PBR-then-posterize shader, ink edges, the felling fade) lives in [docs/toon-shading.md](toon-shading.md); the aesthetic intent and which families are cel vs PBR live in [docs/art-direction.md](art-direction.md). Read those before making a prop cel-shaded.

## Which material a prop uses

There are exactly three custom materials plus stock `StandardMaterial`. Pick by family, not per prop. Mixing cel and PBR on adjacent props reads as a rendering bug.

| Material | Type | Covers | Where |
|---|---|---|---|
| `StandardMaterial` | Bevy stock PBR | players/remote rig, all four held tools, held hammer/plan, dropped + held loot bags, building pieces (sticks/wood/stone tiers), wood + iron doors, stone perimeter walls, the flat fallback ground, impact debris (wood chip / stone shard / grass / blood), placement ghosts, distant tree LODs | `assets.rs`, `world.rs` |
| `ToonMaterial` | standalone cel `Material` | ore/vein nodes (all 4 types), trees (bark + solid canopy + dead snags), harvestable hay/tall-grass, every free-standing deployable (workbench, furnace, storage, torch, tool cupboard, sleeping bag) | `src/app/scene/toon.rs` + `assets/shaders/toon.wgsl`. See [toon-shading.md](toon-shading.md). |
| `TerrainMaterial` | standalone PBR `Material` | the live-world biome-splat ground floor only | `src/app/scene/terrain.rs` + `assets/shaders/terrain.wgsl`. See [TerrainMaterial](#terrainmaterial-biome-splat-ground) below. |
| Harvestable hay/tall-grass + grass instancing pipeline | `ToonMaterial` (hay clump) + a custom render pipeline (detail grass) | the harvestable hay/tall-grass clumps and the GPU-instanced cosmetic grass field | hay is `hay_tall_grass_material` -> `hay_grass_materials: [Handle<ToonMaterial>; 3]` in `assets.rs`; cosmetic grass is `GrassInstancingPlugin` with a binding-free shader (no material asset) in `src/app/scene/grass/`. Out of scope here; see [art-direction.md](art-direction.md) and [profiling.md](profiling.md). |

`MaterialPlugin::<TerrainMaterial>` and `MaterialPlugin::<ToonMaterial>` are each registered **once** in `add_third_party_plugins` in `src/app.rs` (search `add_plugins(MaterialPlugin`). They are client-only: the dedicated server has no render app, so nothing instantiates them there. A scene test that runs `setup_scene` must `app.init_asset::<ToonMaterial>()` / `init_asset::<TerrainMaterial>()`.

> Ore note: ore/vein nodes are `ToonMaterial`, not `StandardMaterial`. Older docs and the obsolete "iron ore 0.78/0.18 metallic" table row described a pre-cel `StandardMaterial` ore that no longer exists. The ore shares one cel material across all four types; per-mineral identity rides entirely on the glb `COLOR_0`, and the cel shader forces matte internally (roughness 0.95, reflectance 0, metallic 0). It receives shadows. Do not reintroduce a per-mineral specular shine.

## The PBR triplet: Bevy defaults are wrong for almost everything

`StandardMaterial::default()` is `metallic: 0.0, perceptual_roughness: 0.5, reflectance: 0.5`: a dielectric with ~4% specular at normal incidence. Fine for plastic, wrong for stone, wood, cloth, leather, dirt. Fresnel also pushes that specular toward 100% at grazing angles, which is what makes flat surfaces read as wet glass under a low sun.

Rules for a new `StandardMaterial`:

1. **Metal is binary.** Real materials are fully metallic (`metallic: 1.0`) or fully dielectric (`metallic: 0.0`). Use an in-between value only for deliberate artistic intent (e.g. visible metal flecks in a stone matrix); the iron door's 0.8 is the only metallic structure surface that ships.
2. **Pick roughness honestly.** Glossy is `< 0.5`, matte is `0.85+`. In this art style almost everything is matte; when in doubt, `0.9`.
3. **Always set `reflectance` explicitly for dielectrics.** `0.1`-`0.2` for matte natural surfaces. `0.5` (the default) is plastic and must be a deliberate choice. A too-high reflectance now also picks up visible sky reflections from the IBL (see [Atmosphere and IBL](#atmosphere-and-ibl)).
4. **Large flat surfaces** the sun can graze (floors, water, tabletops, roofs) need per-vertex normal jitter, not just high roughness. See [Flat surfaces](#flat-surfaces-and-the-wet-glass-band).

## Live per-surface PBR values

These are the values set in code (PBR families only). Treat as the live reference and update this table when you change a material.

| Surface | `perceptual_roughness` | `reflectance` | `metallic` | Source |
|---|---|---|---|---|
| Flat fallback ground (menu backdrop + asset-less tests) | `1.0` | `0.0` | `0` | `world.rs - flat_ground_material` |
| Stone perimeter wall | `0.95` | `0.1` | `0` | `world.rs - stone_material` |
| Building piece, sticks tier | `0.95` | `0.12` | `0` | `assets.rs - building_material` (uv_scale 1.0) |
| Building piece, wood tier | `0.92` | `0.13` | `0` | `assets.rs - building_material` (uv_scale 1.0) |
| Building piece, stone tier | `0.95` | `0.1` | `0` | `assets.rs - building_material` (uv_scale 0.5) |
| Wood (hewn) door | `0.9` | `0.13` | `0` | `assets.rs - hewn_door_material` |
| Iron door (the one metallic structure) | `0.55` | _default 0.5_ | `0.8` | `assets.rs - iron_door_material` |
| Player remote (skin/cloth) | `0.92` | `0.2` | `0` | `assets.rs - remote_material` |
| Dropped loot bag | `0.95` | `0.15` | `0` | `assets.rs - dropped_material` |
| Held loot bag | `0.88` | `0.15` | `0` | `assets.rs - held_bag_material` |
| Held vertex prop (hammer, plan) | `0.90` | `0.15` | `0` | `assets.rs - held_vertex_material` |
| Wood chip debris | `0.95` | `0.12` | `0` | `assets.rs - wood_chip_material` |
| Stone shard debris | `0.88` | `0.12` | `0` | `assets.rs - stone_shard_material` |
| Grass blade debris | `0.92` | `0.12` | `0` | `assets.rs - grass_blade_material` |
| Blood spray | `0.7` | `0.2` | `0` | `assets.rs - blood_material` (slight wet sheen) |
| Placement ghost (valid/invalid) | `0.85` | `0.10` | `0` | `assets.rs - ghost_*_material` (Blend + emissive) |

**Iron door** is the one slightly-metallic structure: `metallic: 0.8` with no explicit reflectance, base color white, the dark `door_iron` texture driving the F0 tint, so the forged plate picks up the sky IBL and reads as steel rather than flat dark.

**Held tools** carry materials baked into their authored Blender glbs, not literal triplets set in `assets.rs` (`glb_material(&glb, mesh_index)` loads them). Iron tools render as two overlaid meshes so only the head layer is fully metallic (`metallic: 1.0`, low roughness) and picks up the IBL as bright steel, while the handle layer stays matte. The grey iron vertex colours drive F0; do not add a base color to a metal. See [docs/playbooks/art-pipeline.md](playbooks/art-pipeline.md) for the glb authoring pipeline.

## Vertex-colour albedos are linear: pick them physically

Low-poly props (sticks, surface stones, tree LODs, impact debris, glb `COLOR_0`) carry colour in `Mesh::ATTRIBUTE_COLOR` multiplied into a white material. **That attribute is linear RGBA**: it never goes through the sRGB decode that `Color::srgb` performs, so a value eyeballed as a perceptual mid-grey (`0.55`) renders ~1.5-2x brighter (chalk white in daylight). This bit the prop set once: every rock, trunk, and stick rendered pastel until the palette was rebuilt in linear terms.

Calibration:

- The anchor is the ground at linear `(0.027, 0.095, 0.040)` (this is `WORLD_COLOR = Color::srgb(0.18, 0.34, 0.22)` decoded). Anything that should "sit in the scene" belongs within a few multiples of that.
- Physical linear albedo ranges: coal `0.02`-`0.05`, dark soil/bark `0.03`-`0.09`, rock `0.08`-`0.26`, foliage green `0.06`-`0.22`, dry straw `0.2`-`0.3`, paper-birch white `~0.5` tops.
- Quick conversion from an sRGB pick: raise to `~2.2` power, then tune by eye in the headless harness (see [docs/headless-agent-testing.md](headless-agent-testing.md)).
- Identity must live in something readable at gameplay distance, not tiny accents. The ore/vein nodes carry it entirely in the studded mineral chunks (iron rust, coal near-black, sulfur yellow, stone-vein plain knobs); the rock body is one shared bright grey on every node. The "tint the whole mound toward the mineral" look read as a coloured blob and was rejected.

The mesh builder bakes cheap fake ambient occlusion in the same linear currency (`scale_rgb` in `src/app/scene/mesh/builder.rs`). Authored glbs (tools, deployables, ore, trees) got the matching `v^2.2` correction pass; metal greys are left untouched because a metal's colour drives F0, not albedo.

## Atmosphere and IBL

The main camera (`assets.rs`, in the `commands.spawn` block alongside `MainCamera`) carries a procedural `Atmosphere::earthlike` plus an `AtmosphereEnvironmentMapLight`. The atmosphere renders the sky **and** generates an environment map from it each frame, which feeds every material's ambient diffuse and specular reflections: the "free IBL" that makes the scene read as genuinely lit, not flat-ambient.

Key camera settings on that entity:

- `Atmosphere::earthlike`, with trimmed `AtmosphereSettings` LUT sample counts for GPU cost (the atmosphere recomputes its LUTs every frame in Bevy 0.18).
- `AtmosphereEnvironmentMapLight { intensity: ATMOSPHERE_AMBIENT_INTENSITY (= 1.0), size: ATMOSPHERE_ENV_MAP_SIZE (= 64) }`. The cubemap is refiltered every frame with no skip-if-unchanged gating, so **its size is the dominant GPU cost lever** (the default 512 cost ~500->70 fps). 64px is visually indistinguishable because the materials are matte and there are no mirrors. Raise it only if a glossier material is added. See [docs/profiling.md](profiling.md).
- `Tonemapping::AgX`: a flat, desaturated, painterly filmic curve. (A stale inline comment in the spawn block mentions TonyMcMapface; the code sets AgX.)
- `Hdr` (bloom + atmosphere both require it), `Msaa::Off`, and `NoIndirectDrawing` (the binned opaque phase intermittently dropped whole batches with indirect drawing once the grass pipeline shared the phase; direct draw is stable, and macOS Metal has limited multi-draw-indirect support).

Consequences for material authoring:

- Ambient now has direction and colour (sky above is brighter/bluer than the ground bounce), so reflectance reads more naturally than under a flat ambient term. But a too-high reflectance now picks up visible sky reflections. Keep the matte table values.
- Daytime ambient comes from the atmosphere. The `GlobalAmbientLight` resource is now only a **night floor** (`sky.rs`) that fades to zero by day. Do not reintroduce a large daytime `GlobalAmbientLight`; it double-lights against the env map and washes the scene out.
- The sun `DirectionalLight` is kept neutral white; the atmosphere tints it warm at the horizon. Do not re-add a per-time-of-day warm tint to the light colour, or it double-counts.

### Brightness is daylight-calibrated, not physical raw sunlight

Brightness is intentionally **not** physical raw-sunlight (~130k lux) + manual exposure. Raw sunlight has too much dynamic range across the day for a single fixed exposure, and the usual fix (auto-exposure) fights the fixed, gameplay-fair night this game wants (it would brighten the dark so the player always sees). Instead the sun sits at a daylight-calibrated `SUN_PEAK_ILLUMINANCE = 11_000.0` lux under the renderer's default exposure, which the atmosphere still renders and tints correctly and which holds a consistent look from dawn to dusk. So keep new lights/emissives in the established scale; do not switch one to physical raw values.

Brightness knobs:

- `ATMOSPHERE_AMBIENT_INTENSITY` (`assets.rs`): daytime ambient/reflection strength.
- `SUN_PEAK_ILLUMINANCE` (`11_000.0`), `MOON_PEAK_ILLUMINANCE` (`1_300.0`), `NIGHT_AMBIENT_FLOOR` (`90.0`) in `sky.rs`: sun/moon/night balance.

## Flat surfaces and the wet-glass band

A perfectly planar mesh has identical normals across the whole face. Under a directional light that produces one continuous Fresnel-driven specular band whose intensity depends only on view + light + surface angle; even at `roughness: 1.0` you still see it at grazing angles, because Fresnel ignores roughness for the F0 term.

`build_ground_mesh` (`world.rs`) solves this: 128 subdivisions plus deterministic multi-frequency sine noise applied **only to the per-vertex normals** (positions stay flat, so movement and collision are untouched). This breaks the mirror-uniform highlight into mottled patches.

Apply the same recipe to any future large flat ground/water/floor surface. Curved or faceted low-poly meshes (trees, ore chunks, bags) already break up the highlight geometrically and need no jitter.

## Shared texture loader: build_mip_chain + tree_texture_sampler

Bevy 0.18 generates **no mips** for loaded PNGs and ships no runtime mip util, so embedded PNG textures alias into shimmer at distance unless mipped on the CPU. Two helpers handle this and are shared across every textured prop:

- `build_mip_chain` (`pub(crate)` in `src/app/scene/terrain.rs`): box-downsamples a full mip chain into `image.data` and sets `mip_level_count`. The format is `Rgba8UnormSrgb`, so colour channels are averaged in **linear** space (decode -> average -> re-encode); alpha is averaged linearly. Costs ~1 ms per texture at startup.
- `tree_texture_sampler` (`scene/materials.rs`): a repeat + `anisotropy_clamp: 8` `ImageSamplerDescriptor`, applied to every loaded detail texture.

This pair loads the tree bark/foliage, the ore rock grain, the deployable wood/stone/fabric masters, and the building/door tier textures, all decoded synchronously at startup. Use it for any new textured prop. (PNG only: the game build enables Bevy's `png` image feature, not `jpeg`.)

LINEAR vertex-colour gotcha: textures load sRGB (auto-decoded by `Rgba8UnormSrgb`), but glb `COLOR_0` / `Mesh::ATTRIBUTE_COLOR` stay linear (see [Vertex-colour albedos](#vertex-colour-albedos-are-linear-pick-them-physically)). Do not double-correct.

## TerrainMaterial (biome splat ground)

The live-world floor is textured by biome so it reads like the world map: forest floor, dry plains grass, rocky ground, ore-vein dirt, cross-fading at biome borders. It is a standalone PBR `Material` (`src/app/scene/terrain.rs` + `assets/shaders/terrain.wgsl`). Only **live** worlds use it; the menu backdrop and asset-less tests fall back to the flat `StandardMaterial` (`GroundMaterial::Flat` in `world.rs`).

How it works:

- **Four shared tileable biome textures** (`assets/textures/terrain/{forest,rocky,ore,plains}.png`) are decoded once into `TerrainTextureAssets` with a CPU mip chain (`build_mip_chain`) and repeat-sampled with anisotropic filtering in world space (`TERRAIN_TILE_SIZE_M` metres per repeat). Neighbouring biomes share one continuous grain with no per-tile seams, and the floor stays crisp into the distance.
- **A per-world biome-weight raster** is baked on the CPU from the seed by `crate::world::render_terrain_weight_rgba` (`src/world/terrain_texture.rs`), using the same `ClassificationChannels` noise the map and live generation use, so ground, map, and resource layout agree. It stores soft weights (`R=forest, G=rocky, B=ore, A=plains`) with a roughly 12-18 m cross-fade band (`TERRAIN_BIOME_BLEND_BAND = 0.03`); `Rgba8Unorm` (linear data) with a clamp+linear sampler and **no** mips (mipping a low-frequency LUT would bleed biome borders). The shader blends the four textures by these weights. See [docs/worlds-and-saves.md](worlds-and-saves.md) for the classification noise.
- **Matte, like the rest of the ground**: the shader hand-builds the `PbrInput` with `perceptual_roughness 1.0`, `reflectance vec3(0.0)`, `metallic 0.0`, so the flat floor never shows the Fresnel band. Lit by the same sun + atmosphere IBL as everything else, and it keeps the shadow-receiver bit (`pbr_input.flags = mesh[in.instance_index].flags`) so it takes tree/building shadows.
- **Anti-tiling + distance fade**: the shader (a) domain-warps the detail UV by a small bounded fbm **offset** (never a rotation of the global coordinate, which smears into radial streaks far from the origin: the bug that bit the first attempt), (b) lays a low-frequency macro brightness wash over the blended albedo, and (c) distance-fades the tiled detail toward the flat biome-map palette so far terrain resolves to the colours the map shows. The fade target is the palette in **linear** space, because `textureSample` of an `Rgba8UnormSrgb` texture already returns linear.

### Metal bind-group rules (shared with ToonMaterial)

`TerrainMaterial` and `ToonMaterial` are both **standalone `Material`s, not `ExtendedMaterial`**. Two non-negotiable rules apply to any new custom-bound material shader:

- **Declare bindings with `@group(#{MATERIAL_BIND_GROUP})`, never a literal `@group(2)`.** In Bevy 0.18 the per-object mesh array lives at the literal `@group(2)` (`mesh_bindings`) and the material bind group is group 3 via that shaderdef. Hardcoding `2` collides with the mesh binding (a runtime "Bindings conflict" naga error). Applies to `terrain.wgsl` and `toon.wgsl` only. `grass_instanced.wgsl` uses a literal `@group(3)` because its custom pipeline (not a Bevy `Material`) owns that group directly.
- **Use a standalone `Material`, not `ExtendedMaterial`, for any custom binding.** `ExtendedMaterial`'s bind-group merge with the bindless `StandardMaterial` drops the extension bindings on Metal at pipeline creation. A standalone material owns its bind group outright, so its texture bindings survive. The cost is the shader rebuilds the `PbrInput` by hand (mirroring `pbr_input_from_standard_material`'s vertex-output half) instead of calling that helper, and must not import `bevy_pbr::pbr_fragment` (which would pull in a second material-group layout and collide).

Heightmap displacement is intentionally not implemented yet. When it lands it attaches to the already-128-subdivided ground mesh and feeds slope into the blend; nothing in this material needs a redesign for it.

## Related docs

- [docs/toon-shading.md](toon-shading.md) - the cel `ToonMaterial` shader mechanics, per-family params, ink edge, felling fade; everything cel-shaded.
- [docs/art-direction.md](art-direction.md) - the visual target and the cel-vs-PBR family boundary.
- [docs/worlds-and-saves.md](worlds-and-saves.md) - biome classification that drives the `TerrainMaterial` weight raster.
- [docs/profiling.md](profiling.md) - why the env map is 64px and the other render-cost levers.
- [docs/playbooks/art-pipeline.md](playbooks/art-pipeline.md) - authoring a glb model or icon (held tools, deployables, ore).
- [docs/headless-agent-testing.md](headless-agent-testing.md) - driving the running game to screenshot and verify a material change.
