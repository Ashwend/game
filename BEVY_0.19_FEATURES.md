# Bevy 0.19 feature backlog

New 0.19 rendering features chosen to adopt during the Bevy 0.18 to 0.19 upgrade,
deferred until the upgrade itself is validated (2-client `multiplayer-test` for
the networking rewrite, plus an in-game visual pass). Branch: `bevy-0.19-upgrade`.

Each entry lists the exact 0.19 API, where it plugs into this codebase, and the
caveats to watch. All four picked features are opt-in; the "free wins" section
lists gains that apply automatically with no code.

Codebase touch points shared by several entries:

- Main world camera spawn: `src/app/scene/assets.rs` (the `MainCamera` bundle).
- Sun / moon directional lights: `src/app/scene/sky.rs` (`setup_sky`).
- Graphics settings + toggles: `src/app/state/settings/data.rs` (the setting
  fields + defaults), `src/app/ui/options/graphics_tab.rs` (the UI checkbox),
  `src/app/systems/graphics.rs` (`apply_graphics_settings_system`, which owns
  bloom / shadows / atmosphere and is the natural place to apply new camera-level
  toggles).
- Renderer / wgpu wiring: `src/app.rs` (the `DefaultPlugins` / `RenderPlugin`
  setup).
- Existing custom prepass: `assets/shaders/toon_prepass.wgsl` + `src/app/scene/toon.rs`
  (the toon materials already run a depth-only prepass entry).

## Shared prerequisite: DepthPrepass on the main camera

Contact shadows AND occlusion culling both require a
`bevy::core_pipeline::prepass::DepthPrepass` on the world camera; without it the
`ContactShadows` / `OcclusionCulling` components are ignored. Add it once on the
`MainCamera` bundle in `assets.rs` and both features can share it.

Validate carefully when adding it:

- The toon materials already provide a custom prepass fragment
  (`toon_prepass.wgsl`, so alpha-masked grass cards discard correctly). Confirm
  the `DepthPrepass` composes with that entry and does not double-draw or break
  the alpha-mask discard.
- Grass draws in `Transparent3d` with `NoFrustumCulling` and writes depth; confirm
  it still renders and is not wrongly culled once occlusion culling is on.
- The first-person viewmodel camera is a separate camera (render layer 1); the
  prepass belongs on the world camera only.

## 1. Contact shadows

Screen-space contact shadows that fill the near-contact gap shadow maps (including
PCSS) miss. Cheap polish that reads as "grounded" under the cel look, for feet,
building pieces, deployables, and ore.

- API: `bevy::pbr::ContactShadows` component on the camera (fields
  `linear_steps: u32`, `thickness: f32`, `length: f32`; implements `Default`),
  plus the `DepthPrepass` above. `ContactShadowsPlugin` is included by the PBR
  plugins.
- Per light opt-in: set `contact_shadows_enabled: true` on the sun
  `DirectionalLight` in `sky.rs` (the field already exists in the 0.19 light
  struct). Composes with the existing `shadow_maps_enabled` + `soft_shadow_size`
  (PCSS) on that light.
- Where: add `ContactShadows` to the `MainCamera` bundle (`assets.rs`), gate it
  behind a new graphics setting (`data.rs` + `graphics_tab.rs`), and apply it in
  `apply_graphics_settings_system` (`graphics.rs`).
- Caveats: cost scales with (lit pixels x number of contact-shadow lights); we
  mostly have one directional sun, so it is cheap. Verify it reads well against
  the toon posterize step (it should sit under the cel bands, not fight them).

## 2. Occlusion culling for bases

Two-phase GPU occlusion culling (early + late depth prepass builds a hierarchical
Z pyramid, then tests mesh bounds). Ideal for the occluder-heavy case: bases with
many walls and foundations. 0.19 also culls directional-light shadow maps.

- API: `bevy::render::...OcclusionCulling` component on the camera + the
  `DepthPrepass` above (required; the component is ignored without it).
- Where: add `OcclusionCulling` to the `MainCamera` bundle (`assets.rs`), behind a
  graphics setting.
- Caveats (important): a known over-cull bug on some hardware (upstream issue
  #19544). Test on the Metal / Apple-Silicon target before defaulting it on; keep
  it behind a toggle initially (default off, opt-in). Confirm the grass field
  (`NoFrustumCulling`, one combined buffer) and any camera-facing billboards do
  not pop when culled.

## 3. Render-error recovery

For a shipped client, a GPU device-lost (driver hiccup, laptop sleep/wake) should
recover rather than crash to desktop.

- API: `bevy::render::error_handler::{RenderErrorHandler, RenderErrorPolicy}`,
  configured via the `RenderPlugin` settings (see `bevy_render` `settings.rs` /
  `error_handler.rs`). Policies: DeviceLost can recover, OutOfMemory can stop
  rendering, Validation can ignore, Internal can panic.
- Where: the `RenderPlugin` / `DefaultPlugins` setup in `src/app.rs`. Set a
  lenient policy in release builds (DeviceLost -> recover); keep the strict
  (panic-on-validation) policy in debug so real bugs still surface.
- Caveats: do NOT silence Validation / OutOfMemory in debug; those are real bugs.
  Low effort, real resilience win. Not visible in normal play, so validate by
  forcing a device-lost (sleep/wake, or a GPU reset) if feasible.

## 4. Vignette / lens post-fx

Optional stylistic accent (a subtle darkened frame edge). Not gameplay.

- API: `bevy::post_process::...Vignette` component on the camera (field
  `intensity: f32`, plus `Default`); a `LensDistortion` and `ChromaticAberration`
  effect also exist in the same `effect_stack` module if wanted.
- Where: add `Vignette` to the `MainCamera` bundle behind a graphics toggle. Tune
  the intensity subtly so it accents rather than darkens the frame; verify it
  composes after bloom / AgX tonemap in the post-process order.
- Caveats: purely cosmetic; keep it off by default or very subtle. Confirm it does
  not fight the flat cel look.

## Free wins (no code, apply automatically)

- Draw-call batching / GPU-driven rendering ("Render Big Scenes Faster"): helps
  the ~1800 AoI entities and dense building-piece bases; biggest gains for
  instances sharing a material/pipeline (the PBR building pieces). Re-benchmark on
  the Metal target, not the upstream NVIDIA numbers.
- Partial bindless on Metal: a free GPU-side win on Apple Silicon for
  material-diverse scenes (many item/building/ore textures).
- Static-transform fast path: building pieces are static-transform heavy and
  benefit automatically.

## Not applicable (do not chase these)

- Improved skinned-mesh culling: does NOT apply. The player rig is a procedural
  `Transform` hierarchy, not skinned glTF (`SkinnedMesh` / `AnimationPlayer` are
  unused), so there is nothing to cull differently.
- Feathers widgets / EditableText / App Settings framework: the UI is egui and
  settings already use `local_crypto`, so these bevy_ui constructs are not worth
  adopting.

## Cross-cutting risks to keep in mind

- Apple-Silicon perf regression (upstream issue #24448) filed against 0.19; the
  batching / bindless wins above should partly offset it, but benchmark before and
  after adopting the render features on the Metal target.
- Both contact shadows and occlusion culling depend on the shared `DepthPrepass`;
  add that first and confirm the toon prepass + grass still render before layering
  either feature on top.
- Bloom now computes luma in linear space (0.19), so it likely needs a re-tune in
  `apply_graphics_settings_system` independently of these features.
