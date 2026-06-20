# Materials

Conventions for Bevy `StandardMaterial` setup in this project. Consult before adding a new material or tweaking an existing one, Bevy's defaults are tuned for plastic, and getting the PBR triplet (reflectance, perceptual_roughness, metallic) wrong is what makes scenes read as "early UE4 demo."

Material setup lives in three places:

- `src/app/scene/assets.rs`, shared material handles for players, items, resource nodes, and impact effects (used everywhere via `Res<...VisualAssets>`).
- `src/app/scene/world.rs`, ground plane and stone perimeter walls (spawned inline as part of `WorldGeometry`).
- `src/app/scene/terrain.rs`, the `TerrainMaterial` that textures the ground floor by biome (see "Terrain ground material" below).

## The defaults are wrong for almost everything

Bevy's `StandardMaterial::default()` is `metallic: 0.0, perceptual_roughness: 0.5, reflectance: 0.5`. That maps to a dielectric with ~4% specular at normal incidence, fine for plastic, wrong for dry/porous/organic materials (stone, wood, cloth, leather, dirt, grass, ore). The Fresnel response also pushes specular to nearly 100% at grazing angles, which is what produces the "wet glass" look on flat surfaces under a low sun.

Rule of thumb: **if it's not actually shiny in the real world, set `reflectance` explicitly.**

## Target values by surface family

| Surface | `perceptual_roughness` | `reflectance` | `metallic` | Notes |
|---|---|---|---|---|
| Ground (live-world terrain floor) | `1.0` | `0.0` | `0.0` | Now the biome-blended `TerrainMaterial` (see "Terrain ground material"). Same matte triplet, just per-biome textured. Menu backdrop + tests keep the flat `StandardMaterial` at these values. Plus per-vertex normal jitter, see "Flat surfaces" below. |
| Stone wall / large rock face | `0.95` | `0.1` | `0.0` | Hint of mineral sheen, no Fresnel pop. |
| Coal, stone vein, generic vertex-coloured natural mesh (ore, surface stones, branch piles, hay grass, tree LODs) | `0.95–0.98` | `0.12` | `0.0` | Porous mineral / dry organic. |
| Dead-snag bark | `0.97` | `0.10` | `0.0` | Pine bark texture × a desaturated cool-grey `base_color` tint, so a leafless trunk reads grey/dead. Bark-trunk glb (`assets/trees/dead_*`). |
| Tree bark (live pine/birch trunk) | `0.95` | `0.12` | `0.0` | Opaque, repeat-tiled `base_color_texture` (mipped), base-white so the glb COLOR_0 tints it. See "Trees" below. |
| Tree canopy (needle/leaf) | `0.95` | `0.12` | `0.0` | `AlphaMode::Mask(0.4)` + `cull_mode: None` (double-sided), mipped `base_color_texture`. Up-biased glb normals so it lights soft, never as a dark wall. |
| Building piece (sticks / hewn wood / stone) + wood door | `0.9–0.95` | `0.1–0.13` | `0.0` | Authored Blender glbs, base-white + repeat-tiled mipped `base_color_texture` per tier/variant (twig, timber, coursed stone; door plank), the glb COLOR_0 tints frame/braces on top. Built in `assets.rs` from `assets/textures/building/*.png`. Source: `art/building/build_pieces.py` + `build_door.py`. |
| Iron door | `0.55` | _default 0.5_ | `0.8` | The one slightly-metallic structure surface, so the forged plate picks up the sky IBL and reads as steel rather than flat dark; the dark `door_iron` texture drives the F0 tint. |
| Sulfur | `0.88` | `0.12` | `0.0` | Brittle chalky yellow, never glossy. |
| Wood chip / stone shard impact debris | `0.88–0.95` | `0.12` | `0.0` | Matches its source material. |
| Grass blade impact | `0.92` | `0.12` | `0.0` | Tinted via `base_color`; the vertex colour multiplies through. |
| Leather/cloth (dropped bag, held bag) | `0.88–0.95` | `0.15` | `0.0` | Matte cloth/leather. |
| Tool handle / wood + metal blend (stone tools, iron tool bodies) | `0.92` | `0.15` | `0.0` | Held tools are constantly in view, so this one matters more than its size suggests. Also used for the matte handle layer of the iron tools. |
| Forged iron tool head (iron hatchet/pickaxe head layer) | `0.34` | _default 0.5_ | `1.0` | The shiny one. Iron tools render as two overlaid meshes so only the head gets this fully-metallic, low-roughness material, picking up the sky/IBL as bright steel while the handle stays matte. Grey iron vertex colours drive F0; don't add a base colour. |
| Player remote (skin/cloth) | `0.92` | `0.2` | `0.0` | A touch of life, still matte. |
| Iron ore (ore with visible metal content) | `0.78` | _default 0.5_ | `0.18` | The one intentional shine, sells "there's metal in this rock." Don't drop reflectance here; the metallic term governs F0 instead. |

These are the values currently set; treat them as the live reference and update this table when you change a material.

## When you add a new material

1. Decide if it's a metal. Real-world metals are either fully metallic (`metallic: 1.0`) or fully dielectric (`metallic: 0.0`). Use the in-between range only for explicit artistic intent like the iron ore above (visible metal flecks in a stone matrix).
2. Pick roughness honestly. A glossy material has roughness `< 0.5`; a matte one is `0.85+`. In this art style almost everything is matte, when in doubt, `0.9`.
3. **Always set `reflectance` explicitly for dielectrics.** `0.1–0.2` for matte natural surfaces, `0.3–0.4` for polished wood / smooth ceramic if you ever want it, `0.5` is plastic and should be a deliberate choice.
4. If the new mesh is a **large flat surface** that the sun can grazingly hit (floors, water, table tops, roof tiles), do not rely on roughness alone to kill the specular band. See below.

## Vertex-colour albedos are linear, pick them physically

The low-poly props (ore nodes, sticks, stones, the tree LODs; `src/app/scene/mesh/`) carry their colour in `Mesh::ATTRIBUTE_COLOR`, multiplied into a white `StandardMaterial`. **That attribute is linear RGBA**, it never goes through the sRGB decode `Color::srgb` performs, so a value eyeballed as a perceptual mid-grey (`0.55`) actually renders ~1.5-2x brighter (chalk white in daylight). This bit the prop set once already: every rock, trunk, and stick rendered pastel until the palette was rebuilt in linear terms.

When picking new values:

- The calibration anchor is the ground at linear `(0.027, 0.095, 0.040)`. Anything that should read as "sits in the scene" belongs within a few multiples of that.
- Physical albedo ranges (linear): coal `0.02-0.05`, dark soil/bark `0.03-0.09`, rock `0.08-0.26`, foliage green channel `0.06-0.22`, dry straw `0.2-0.3`, paper-birch white `~0.5` tops.
- Quick conversion if you have an sRGB pick: raise it to ~2.2 power, then tune by eye in the headless harness (spawn the node with `/spawn`, screenshot at 4-10m).
- Identity needs to live in the *mass* of a node, not in small accents: the per-ore tint is on the whole base rock (see `OreNodeStyle` in `mesh/ore.rs`), because the embedded chunks alone are unreadable past ~8m.

The mesh builder bakes cheap fake ambient occlusion in the same currency: cone bottom caps at 0.45x, octa-rock undersides at 0.58x, rock-lump ground-contact bands at 0.72x (`scale_rgb` in `mesh/builder.rs`). Per-instance size variety comes from a deterministic node-id-hashed uniform scale jitter in `resource_node_transform_at`, not from mesh duplicates.

The held tools and the vertex-coloured placed structures (workbench, furnace, storage boxes, tool cupboard, torch) are authored Blender glbs with their own baked `COLOR_0` values; they got the matching correction pass (dielectric colours `v^2.2`; iron-head grays untouched since a metal's colour drives F0, not albedo; the furnace's AO-baked colours took a flat warm-biased scale instead, see [Icon to 3D model](icon-to-model.md#vertex-colour-albedos)).

The **building pieces and door panels** are also authored glbs (`art/building/build_pieces.py` + `build_door.py`), but they follow the **tree pattern** instead: base-white **textured** materials (repeat-tiled mipped `base_color_texture`, one per building tier + per door variant) with the glb COLOR_0 only tinting the frame/braces/under-structure on top, not carrying the whole look. The geometry mirrors the same box layout as the collider (`crate::building::piece_local_boxes`), so the textured visual still agrees with what blocks movement, and the sticks tier keeps its open lashed-pole lattice so the three tiers stay distinct by silhouette as well as surface.

## Flat surfaces and the "wet glass" problem

A perfectly planar mesh has identical normals across the whole face. Under a directional light, that produces one continuous Fresnel-driven specular band whose intensity depends only on view + light + surface angle, even with `roughness: 1.0` you still see it at grazing angles, because Fresnel ignores roughness for the F0 term.

The ground plane in `src/app/scene/world.rs` solves this with `build_ground_mesh`: 128 subdivisions plus deterministic multi-frequency sine noise applied **only to the per-vertex normals** (positions stay flat so movement and collision are untouched). This breaks the otherwise mirror-uniform highlight into mottled patches.

Apply the same recipe to any future large flat ground/water/floor surface. For curved or faceted low-poly meshes (trees, ore chunks, bags), the geometry already breaks up the highlight and no normal jitter is needed.

## Terrain ground material (biome splat blend)

The live-world floor is textured by biome so it reads like the world map: forest floor, dry plains grass, rocky ground, and ore-vein dirt, cross-fading at biome borders. It's the project's second custom material (after grass), in [`src/app/scene/terrain.rs`](../src/app/scene/terrain.rs) with the shader at [`assets/shaders/terrain.wgsl`](../assets/shaders/terrain.wgsl). Only **live** worlds use it; the menu backdrop and asset-less unit tests fall back to the flat `StandardMaterial` (the `GroundMaterial` enum in `world.rs`).

How it works:

- **Four shared tileable biome textures** (`assets/textures/terrain/{forest,rocky,ore,plains}.png`, generated with the `lowpoly-game-assets` skill; **PNG** because the game build only enables Bevy's `png` image feature, not `jpeg`) are decoded once into `TerrainTextureAssets` (`Image::from_buffer` on the embedded bytes) with a **CPU-built mip chain** (`build_mip_chain`, since Bevy 0.18 generates no mips for loaded images) and repeat-sampled with **anisotropic filtering** in world space (`TERRAIN_TILE_SIZE_M` metres per repeat). Neighbouring biomes share one continuous grain with no per-tile seams, and the floor stays crisp into the distance instead of aliasing into shimmer.
- **A per-world biome-weight raster** is baked on the CPU from the seed by `crate::world::render_terrain_weight_rgba` (in `src/world/terrain_texture.rs`), using the *same* `ClassificationChannels` noise the map and live generation use, so the ground, the map, and the actual resource layout all agree. It stores soft weights (`R=forest, G=rocky, B=ore, A=plains`) with crisp biome interiors and a ~30-40 m cross-fade band; `Rgba8Unorm` (linear, it's data) with a clamp+linear sampler, **no** mips (it's a low-frequency LUT; mipping it would bleed biome borders). The shader blends the four textures by these weights.
- **Matte, like the rest of the ground**: `perceptual_roughness 1.0`, `reflectance 0.0`, so the flat floor never shows the Fresnel "wet glass" band. Lit by the same sun + atmosphere IBL as everything else.
- **Distance + anti-tiling** (so the tiled grain doesn't read as a repeat, and the far field doesn't shimmer): the shader (a) domain-warps the detail UV by a small bounded fbm **offset** (never a rotation of the global coordinate, which amplifies with distance from the origin and smears the texture into radial streaks, the bug that bit the first attempt), (b) lays a low-frequency macro brightness wash over the blended albedo (the grass shader's `hash12`/`value_noise`/`fbm2`), and (c) **distance-fades the tiled detail toward the flat biome map palette** (`params.z`/`params.w` window) so far terrain resolves to the same flat colours the map shows. Mips + anisotropy own the mid-range; the fade owns the far. The fade target is the map palette in **linear** space (the `PAL_*` consts), because `textureSample` of an `Rgba8UnormSrgb` texture already returns linear.

Why a **standalone `Material`** and not an `ExtendedMaterial<StandardMaterial, _>` like grass: a standalone material owns the material bind group outright, so its texture bindings survive on Metal. The grass extension is binding-free precisely because Bevy 0.18's bindless-`StandardMaterial` bind-group merge drops extension bindings on Metal (see the grass notes above), which would break texture sampling here. The cost is the shader rebuilds the `PbrInput` by hand (mirroring `pbr_input_from_standard_material`'s vertex-output half) instead of calling that helper; importantly it reads `mesh[in.instance_index].flags` so the floor keeps the shadow-receiver bit and still takes tree/building shadows. It does **not** import `bevy_pbr::pbr_fragment`, which would pull in `pbr_bindings`' own material-group layout and collide. **Declare the material's bindings with `@group(#{MATERIAL_BIND_GROUP})`, not a literal `@group(2)`**: in Bevy 0.18 the per-object mesh array lives at the literal `@group(2)` (`mesh_bindings`) and the material bind group is group **3** via that shaderdef, so hardcoding `2` collides with the mesh binding (a runtime "Bindings conflict" naga error).

Heightmap note: displacement is intentionally not implemented yet. When it lands it attaches to the already-128-subdivided ground mesh (`build_ground_mesh`) and would feed slope into the blend; nothing in this material needs a redesign for it.

## Environment lighting (IBL)

The camera carries a procedural `Atmosphere` plus `AtmosphereEnvironmentMapLight` (set up in [`assets.rs`](../src/app/scene/assets.rs)). The atmosphere renders the sky **and** generates an environment map from it each frame, which feeds every material's ambient diffuse and specular reflections, the "free IBL" that makes the scene read as genuinely lit. This replaced the old hand-authored `ClearColor` sky and the all-ambient-term lighting model.

Consequences for material authoring:

- Ambient now has *direction and colour* (sky above is brighter/bluer than the ground bounce), so reflectance values read more naturally than they did under a flat ambient term, but it also means a too-high `reflectance` will now pick up visible sky reflections. Keep the matte values in the table above.
- Daytime ambient comes from the atmosphere; the `GlobalAmbientLight` resource is now only a **night floor** (see [`sky.rs`](../src/app/scene/sky.rs)) and fades to zero by day. Don't reintroduce a large day-time `GlobalAmbientLight`, it double-lights against the environment map and washes the scene out.
- The sun `DirectionalLight` is kept neutral white; the atmosphere tints it toward warm at the horizon. Don't re-add a per-time-of-day warm tint to the light colour or it double-counts.

Brightness is intentionally **not** physical raw-sunlight + manual exposure. Raw sunlight (~130k lux) has too much dynamic range across the day for a single fixed exposure to look good, and the usual fix, auto-exposure, fights the fixed, gameplay-fair night this game wants (it would brighten the dark so the player always sees). Instead the sun sits at a daylight-calibrated `SUN_PEAK_ILLUMINANCE` (≈ `AMBIENT_DAYLIGHT`) under the renderer's default exposure, which the atmosphere still renders/tints correctly and which holds a consistent, hand-tuned look from dawn to dusk. So existing lights/emissives stay in the established scale, don't switch a new one to physical raw values.

Brightness knobs: `ATMOSPHERE_AMBIENT_INTENSITY` (in `assets.rs`) for daytime ambient/reflection strength, and `SUN_PEAK_ILLUMINANCE` / `NIGHT_AMBIENT_FLOOR` / `MOON_PEAK_ILLUMINANCE` in `sky.rs` for sun/night balance.

## Detail grass (GPU-instanced, the one custom render pipeline)

The procedural detail grass ([`src/app/scene/grass/`](../src/app/scene/grass/)) is drawn by the project's **only** custom render pipeline ([`instancing.rs`](../src/app/scene/grass/instancing.rs) + [`assets/shaders/grass_instanced.wgsl`](../assets/shaders/grass_instanced.wgsl)), following Bevy 0.18's `examples/shader_advanced/custom_shader_instancing.rs`. One shared cubic-Bézier blade mesh is drawn thousands of times from a per-blade instance buffer, so the field can be dense (150 blades/m² at Medium) for almost no per-blade cost. Each blade instance carries `[world_x, world_z, base_y, height_scale]` + `[yaw, shade, warm, dither]` (vertex `@location` 3/4). Grass **colour is uniform** across biomes (one slightly warm/dry green set on the blade mesh via `DETAIL_GRASS_DRY`, so it sits on both the green forest floor and tan plains); a per-biome colour tint was tried and removed (too subtle to read at eye level). What *does* vary by biome is **density**: `tile_world_instances` samples `biome_blend_weights` per blade and thins the field on bare rock/ore (`biome_grass_barrenness` × `GRASS_BIOME_MAX_THIN`). Seedless fields (menu backdrop) keep full density.

It is **lit by the same sun + atmosphere IBL as everything else** without a material bind group: the pipeline specialises off `MeshPipeline` (so it inherits the mesh-view bind groups, lights/shadows/globals/atmosphere), and the fragment hand-builds a `PbrInput` and calls `apply_pbr_lighting` + `main_pass_post_lighting_processing`. On top of PBR it adds vertex wind sway (weighted by vertex-colour alpha, 0 base → 1 tip), a fragment radial dither (whole-blade discard keyed on the per-instance `dither`, thinning the field into smooth rings), and a world-space fBm colour-patch tint.

Conventions / gotchas if you touch it or add another render pipeline:

- **One entity, one instance buffer.** All visible blades live in a single field entity's buffer ([`GrassState`](../src/app/scene/grass/mod.rs)), rebuilt as tiles stream in/out. Many entities sharing one mesh collide with Bevy's automatic instancing/batching and render as a single clumped draw, so do **not** spawn one entity per tile.
- **Match the view layout via `ViewKeyCache`.** `queue_grass` reads Bevy's cached per-view `MeshPipelineKey` instead of re-deriving msaa/hdr/atmosphere bits by hand, otherwise the camera's atmosphere IBL bindings (view group, bindings 29-31) are missing from the specialised pipeline and the draw panics (`mesh_view_layout_atmosphere ... not compatible`).
- **`Transparent3d` + `NoIndirectDrawing`.** Blades are opaque (alpha 1 + fragment `discard`) but drawn in the transparent phase with the standard opaque depth state. The draw path uses `draw_indexed` (not indirect), which requires `NoIndirectDrawing` on the camera, already set in [`assets.rs`](../src/app/scene/assets.rs).
- **Sync via `ExtractComponentPlugin`.** The field entity has no `Material`, so nothing else opts it into render-world sync; `ExtractComponentPlugin::<InstanceMaterialData>` both syncs it and extracts the buffer. A custom extract that queries `RenderEntity` finds **nothing** without this (the symptom was grass rendering to an off-screen capture but not the live window). It re-extracts + re-uploads every frame; extracting/uploading only on change is a known future optimisation but needs the entity registered for sync some other way.
- `bytemuck` (`Pod`/`Zeroable`) is a direct dependency for the instance record byte-cast.

### Hay grass (`ExtendedMaterial`, the legacy grass shader)

The harvestable **hay-grass** node still uses `GrassMaterial`, an `ExtendedMaterial<StandardMaterial, GrassWindExtension>` backed by [`assets/shaders/grass.wgsl`](../assets/shaders/grass.wgsl). It's one located, pickable clump per node (not a density problem), so the simpler material path is the right tool.

- In Bevy 0.18 `ShaderRef` lives in `bevy::shader`; `AsBindGroup` is in `bevy::render::render_resource`; `ExtendedMaterial`/`MaterialExtension` in `bevy::pbr`.
- **The extension is binding-free on purpose (`GrassWindExtension {}`).** A bound extension uniform (`#[uniform(100)]`) crashed at pipeline creation on Metal, `ExtendedMaterial`'s bind-group merge with the bindless `StandardMaterial` drops the extension binding from the layout (`ResourceBinding group:2 binding:100 ... missing from pipeline layout`). So that shader's tuning is WGSL `const`s. If you need *dynamic* per-material data, use a **standalone `Material`** (its group-2 layout is yours alone), the bound `ExtendedMaterial` extension is fragile on Metal in 0.18.
- `MaterialPlugin::<GrassMaterial>` and `GrassInstancingPlugin` are both registered in `app.rs` only, the dedicated server has no render app, so nothing instantiates them there.
- Hay grass keeps `cull_mode: None` + `double_sided: false` and upward (+Y) vertex normals so blades read as lit-from-above from any angle, and never cast shadows (`NotShadowCaster`).

## Trees (textured Blender glbs)

The full-detail live trees (pine + birch, three sizes) are authored Blender glbs
generated by the parametric script [`art/trees/build_tree.py`](../art/trees/build_tree.py)
(the script header has the full Draw-Things + OpenCV texture recipe). Each glb is
**two meshes** loaded in [`assets.rs`](../src/app/scene/assets.rs): mesh 0 = the
bark **trunk**, mesh 1 = the needle/leaf **canopy**. Rust builds **four shared
materials** (pine/birch × bark/foliage) and the spawn path
([`resource_nodes/spawn.rs`](../src/app/systems/items/resource_nodes/spawn.rs))
puts the canopy on a child of the trunk, so every instance of a species shares one
mesh + material pair and the forest batches. Conventions that matter:

- **Textures are decoded synchronously with a CPU mip chain** (`build_mip_chain`,
  shared with the terrain loader) and a repeat + anisotropic sampler. Bevy 0.18
  builds no mips for loaded PNGs; without them the alpha-mask canopy aliases into
  sparkle at distance. Bark tiles vertically up the trunk; needles/leaves tile
  across the canopy shells.
- **Bark = opaque**, default back-face cull. **Canopy = `AlphaMode::Mask(0.4)`**
  (never `Blend`, which would sort + overdraw at forest scale) and
  `cull_mode: None` so needles/leaves read from both faces, like the hay tuft.
- **Up-biased canopy normals (`UP_BIAS = 0.5`).** The glb bakes per-vertex normals
  lerped halfway toward +Y so the canopy lights soft from the sky without going
  dark-walled, but still keeps real light/shadow FORM (top brighter, flanks
  darker). An earlier 0.72 washed the form out flat; pure facet normals dark-wall.
  Set in the Blender script, not in Rust.
- **Clumped canopy surfaces (`CONE_JITTER` / `BLOB_JITTER`).** Each cone ring and
  octa-blob vertex is pushed in/out + up/down by a seeded `hash01`, so the canopy
  is a lumpy cluster of needle/leaf clumps with a ragged silhouette instead of a
  smooth lathe surface reading as a textured geometric solid. Birch leaves also
  run more transparent (~64% opaque, vs pine needles ~88%) so the deciduous crown
  reads as see-through layered foliage rather than a flat "jpeg" blob.
- **Vertex colours stay linear, textures load sRGB.** The materials are base-white;
  the glb COLOR_0 tints per canopy layer (dark lower → light crown) and adds the
  trunk's base-ring ground-contact AO. Don't double-correct.
- **The trunk continues up through the canopy** as a thin tapering spine (to
  ~93% of the canopy top, `TRUNK_EXTEND_FRAC` in `build_tree.py`), so it reads as
  a real continuous trunk visible through the foliage gaps, never a stub cut off
  where the branches start. The trunk tube is **capped at both ends** (a fan to a
  centre vertex, `recalc_face_normals` orienting the closed manifold) so a felled
  trunk shows a solid bark end instead of a see-through hollow pipe.
- **Pine cone rims feather via vertex-colour alpha** (bottom ring alpha 0 ramping
  up over ~16% of the cone height, so `Mask` cuts a soft ragged skirt, not a hard
  "party-hat" disc), and each cone has a **soft bottom cap** (opaque centre fading
  to the alpha-0 rim) so the canopy isn't a see-through hollow shell from below /
  during felling. Birch blobs are full octahedra (kept underside, up-biased,
  jittered) so the crown reads as a leafy volume.
- **Shadows:** the trunk and the canopy cast (a forest floor stays shaded up
  close, and the felling tree keeps its full shadow through the fall); only the
  distant low-poly LOD child is `NotShadowCaster`, so trees past `TREE_LOD_DISTANCE`
  (80 m) stop flooding the shadow cascades. The LODs remain vertex-coloured
  `LowPolyMeshBuilder` meshes (retuned `LEAF_*`/`BARK_*` linear constants so the
  80 m hard switch doesn't flip the canopy brightness).
- **Dead snags are bark-trunk glbs too** (`assets/trees/dead_*`): a tapered bark
  trunk + bare branches, no canopy, tinted weathered grey by `dead_bark_material`.
  A felled snag drops a bare trunk (the felling path checks `NetworkResourceNode.dead`
  and skips the canopy), not a sprouting live crown.
