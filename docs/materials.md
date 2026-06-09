# Materials

Conventions for Bevy `StandardMaterial` setup in this project. Consult before adding a new material or tweaking an existing one, Bevy's defaults are tuned for plastic, and getting the PBR triplet (reflectance, perceptual_roughness, metallic) wrong is what makes scenes read as "early UE4 demo."

Material setup lives in two places:

- `src/app/scene/assets.rs`, shared material handles for players, items, resource nodes, and impact effects (used everywhere via `Res<...VisualAssets>`).
- `src/app/scene/world.rs`, ground plane and stone perimeter walls (spawned inline as part of `WorldGeometry`).

## The defaults are wrong for almost everything

Bevy's `StandardMaterial::default()` is `metallic: 0.0, perceptual_roughness: 0.5, reflectance: 0.5`. That maps to a dielectric with ~4% specular at normal incidence, fine for plastic, wrong for dry/porous/organic materials (stone, wood, cloth, leather, dirt, grass, ore). The Fresnel response also pushes specular to nearly 100% at grazing angles, which is what produces the "wet glass" look on flat surfaces under a low sun.

Rule of thumb: **if it's not actually shiny in the real world, set `reflectance` explicitly.**

## Target values by surface family

| Surface | `perceptual_roughness` | `reflectance` | `metallic` | Notes |
|---|---|---|---|---|
| Ground (grass/dirt plane) | `1.0` | `0.0` | `0.0` | Plus per-vertex normal jitter, see "Flat surfaces" below. |
| Stone wall / large rock face | `0.95` | `0.1` | `0.0` | Hint of mineral sheen, no Fresnel pop. |
| Coal, stone vein, generic vertex-coloured natural mesh (trees, surface stones, branch piles, hay grass) | `0.95–0.98` | `0.12` | `0.0` | Porous mineral / dry organic. |
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

## Flat surfaces and the "wet glass" problem

A perfectly planar mesh has identical normals across the whole face. Under a directional light, that produces one continuous Fresnel-driven specular band whose intensity depends only on view + light + surface angle, even with `roughness: 1.0` you still see it at grazing angles, because Fresnel ignores roughness for the F0 term.

The ground plane in `src/app/scene/world.rs` solves this with `build_ground_mesh`: 128 subdivisions plus deterministic multi-frequency sine noise applied **only to the per-vertex normals** (positions stay flat so movement and collision are untouched). This breaks the otherwise mirror-uniform highlight into mottled patches.

Apply the same recipe to any future large flat ground/water/floor surface. For curved or faceted low-poly meshes (trees, ore chunks, bags), the geometry already breaks up the highlight and no normal jitter is needed.

## Environment lighting (IBL)

The camera carries a procedural `Atmosphere` plus `AtmosphereEnvironmentMapLight` (set up in [`assets.rs`](../src/app/scene/assets.rs)). The atmosphere renders the sky **and** generates an environment map from it each frame, which feeds every material's ambient diffuse and specular reflections, the "free IBL" that makes the scene read as genuinely lit. This replaced the old hand-authored `ClearColor` sky and the all-ambient-term lighting model.

Consequences for material authoring:

- Ambient now has *direction and colour* (sky above is brighter/bluer than the ground bounce), so reflectance values read more naturally than they did under a flat ambient term, but it also means a too-high `reflectance` will now pick up visible sky reflections. Keep the matte values in the table above.
- Daytime ambient comes from the atmosphere; the `GlobalAmbientLight` resource is now only a **night floor** (see [`sky.rs`](../src/app/scene/sky.rs)) and fades to zero by day. Don't reintroduce a large day-time `GlobalAmbientLight`, it double-lights against the environment map and washes the scene out.
- The sun `DirectionalLight` is kept neutral white; the atmosphere tints it toward warm at the horizon. Don't re-add a per-time-of-day warm tint to the light colour or it double-counts.

Brightness is intentionally **not** physical raw-sunlight + manual exposure. Raw sunlight (~130k lux) has too much dynamic range across the day for a single fixed exposure to look good, and the usual fix, auto-exposure, fights the fixed, gameplay-fair night this game wants (it would brighten the dark so the player always sees). Instead the sun sits at a daylight-calibrated `SUN_PEAK_ILLUMINANCE` (≈ `AMBIENT_DAYLIGHT`) under the renderer's default exposure, which the atmosphere still renders/tints correctly and which holds a consistent, hand-tuned look from dawn to dusk. So existing lights/emissives stay in the established scale, don't switch a new one to physical raw values.

Brightness knobs: `ATMOSPHERE_AMBIENT_INTENSITY` (in `assets.rs`) for daytime ambient/reflection strength, and `SUN_PEAK_ILLUMINANCE` / `NIGHT_AMBIENT_FLOOR` / `MOON_PEAK_ILLUMINANCE` in `sky.rs` for sun/night balance.

## Detail grass (GPU-instanced, the one custom render pipeline)

The procedural detail grass ([`src/app/scene/grass/`](../src/app/scene/grass/)) is drawn by the project's **only** custom render pipeline ([`instancing.rs`](../src/app/scene/grass/instancing.rs) + [`assets/shaders/grass_instanced.wgsl`](../assets/shaders/grass_instanced.wgsl)), following Bevy 0.18's `examples/shader_advanced/custom_shader_instancing.rs`. One shared cubic-Bézier blade mesh is drawn thousands of times from a per-blade instance buffer, so the field can be dense (150 blades/m² at Medium) for almost no per-blade cost. Each blade instance carries `[world_x, world_z, base_y, height_scale]` + `[yaw, shade, warm, dither]`.

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
