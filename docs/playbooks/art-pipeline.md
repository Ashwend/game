---
title: "Playbook: author a model or icon"
owns: The repeatable model/icon authoring pipeline (hand-modelled held tools, parametric-script glb families, and the skill<->repo boundary).
when_to_read: When modelling a new held item or prop, deriving a sibling tool, authoring a deployable/building glb, or generating an icon/texture.
sources:
  - "src/app/scene/assets.rs - setup_scene asset wiring (the prim_mesh closure in insert_deployable_visuals, ItemVisualAssets, DeployableVisualAssets)"
  - "src/app/systems/items/held.rs - held_item_layers"
  - "src/app/scene/toon.rs - ToonMaterial"
  - "art/ore/ - image-to-3D ore family (prompts, candidates, build_nodes.py)"
  - "art/pipeline/ - family-agnostic tools (render_turntable.py, retopo_experiment.py)"
  - "art/deployables/build_deployables.py - deployable UV bake + workbench rebuild"
  - "scripts/icon_finalize.py - master->in-game icon.png downscale"
related:
  - "docs/toon-shading.md - the cel ToonMaterial that ore, trees, and deployables share"
  - "docs/rendering-materials.md - StandardMaterial PBR conventions and the linear COLOR_0 palette calibration"
  - "docs/items-and-resources.md - how held_mesh / item ids map to the renderer"
  - "docs/playbooks/add-content.md - the surrounding 'add a tool/ore/deployable' steps"
  - "docs/headless-agent-testing.md - the control socket + capture harness used to verify a model in-game"
---

# Playbook: author a model or icon

> When to read this: modelling a new held item or prop, deriving a sibling tool, authoring a deployable/building glb, or generating an icon/texture. Source of truth: `src/app/scene/assets.rs` (setup_scene), `art/*/build_*.py`, `scripts/icon_finalize.py`. Canonical invariants live in CLAUDE.md.

Two facts shape everything below: in-game props get their colour either from a `COLOR_0` vertex-colour attribute baked into the glb (LINEAR albedos, see [rendering-materials.md](../rendering-materials.md)) or, for the image-to-3D rebuilds, from a per-item baked albedo texture over white COLOR_0; and the **material is attached in Rust at load time**, not exported from the model. The icon-generation half (img2img reskins, tileable texture synthesis) lives in the `lowpoly-game-assets` skill, not the repo; the repo owns the build scripts, `icon_finalize.py`, and the `assets.rs` wiring.

## Three authoring tracks

| | Image-to-3D families | Hand-modelled glbs | Parametric headless scripts |
|---|---|---|---|
| What | ore nodes (`art/ore/`), the five gathering tools (`art/held/`); the default lane for future held-item batches | hammer, melee weapons, deployable `.blend`s | trees, building pieces, doors, deployable post-processing |
| How | reference prompt -> RunPod TRELLIS.2 -> `build_*.py` retopo + albedo rebake (see the family section below) | Blender driven live over the Blender MCP; the `.blend` is source of truth | `art/<family>/build_*.py` run headless (`Blender --background --python ...`) |
| `export_materials` | `'NONE'` (the engine attaches the per-item/per-type material) | `'EXPORT'` | trees use `'NONE'`; building, door, and deployable scripts use `'EXPORT'` (Rust attaches/selects the shared material either way) |
| Primitives | 1 (whole prop; held items add a `socket_grip` NODE) | per model | typically 1 (trees = 2: trunk + foliage) |
| Material kind | `ToonMaterial` (+ `ToonViewmodelMaterial` for held) over a baked albedo, COLOR_0 white | shared tool-family cel materials tinted by COLOR_0 | mostly `ToonMaterial`; building pieces + doors are textured `StandardMaterial` |
| Rust load | `prim_mesh` + per-item/per-type material in `assets.rs`; held sockets via `HeldGripSockets` | `prim_mesh` per primitive | `prim_mesh` only; material built/selected in Rust |

Rule of thumb: **do not** hand-model in interactive Blender a shape that must stay in lockstep with a Rust constant (a building piece, a tree silhouette, an ore stage); those go through a parametric script that mirrors the constant. The five gathering tools (stone/iron hatchet + pickaxe, iron sickle) moved to the image-to-3D track 2026-07; their old hand-modelled `.blend`s and the sickle build scripts are retired.

Note the deployables straddle the line: their `.blend` source was hand-authored, but `art/deployables/build_deployables.py` post-processes them (bakes UVs, rebuilds the workbench). Treat them as the parametric track for material/UV purposes.

The Blender MCP (`mcp__Blender__execute_blender_code`, render helpers) is an **external dependency** that may not be connected in a given session. The hand-modelled track requires it; the parametric scripts only need a local Blender binary.

## Held-item reference frame and the grip-socket contract

Every held glb is scaled into one shared reference frame so the shared carry/swing transforms just work:

- pommel at authoring z = `-0.514`, head top ~`+0.356`, total height ~`0.87`
- authoring frame (Blender is Z-up, the glb exports +Y up via `export_yup=True`): handle along Blender `+Z` (in-game up), working edge along Blender `+X` (in-game forward, so it faces the swing), thin axis along Blender `+Y`

For image-to-3D rebuilds this fit is enforced by `art/held/build_held.py` (`HELD_FIT` knobs: up-axis/yaw fixes, height, grip height); nothing is eyeballed.

**The grip-socket contract (Phase 0 of ART-PIPELINE-REWORK).** Every rebuilt held glb carries a `socket_grip` node (a Blender empty parented to the mesh): socket `+Y` runs along the haft toward the head, socket `+Z` faces the working edge, position is the authored grip point on the haft. On a `+X`-edge tool that is a `Rot_Y(PI/2)` node. The engine (`HeldGripSockets` in `src/app/systems/items/held.rs`) resolves the node by name from the loaded Gltf and DERIVES hand placement as `carry_anchor * socket⁻¹` in all three views (first-person, remote rig, paperdoll), so a socketed item needs zero per-item Rust constants; items without a socket keep the legacy constant path. A contract socket at the mesh origin reproduces the legacy tool pose exactly (regression-tested in `held.rs`).

### COLOR_0 albedos are LINEAR

`COLOR_0` bypasses the sRGB decode, so a value picked "by eye" as a mid tone renders ~1.5-2x too bright. Author against the linear palette in `src/app/scene/mesh/builder.rs` (calibration note at the top), not perceptually. Two extra rules for the tools:

- **Metal head slots are exempt.** A metal's vertex colour drives F0 (the mirror tint), not diffuse albedo; the iron heads' bright greys (`0.38-0.94`) are correct and must not be darkened with the dielectric values.
- **Bias stone tones warm** (R above B). Cool neutral greys amplify the blue-sky IBL into a washed pale haze on large surfaces.

`COLOR_0` exports **white** unless the "Color" attribute is the active *render* colour attribute (`mesh.color_attributes.render_color_index`) **and** each material wires a Vertex Color node into Base Color. Verify the exported glb carries a non-white `COLOR_0` per primitive before trusting it.

### Export and Blender MCP gotchas

```python
bpy.ops.export_scene.gltf(
    filepath="assets/items/<id>/model.glb",
    export_format='GLB', use_selection=True, export_yup=True,
    export_apply=True, export_normals=True,
    export_materials='EXPORT', export_texcoords=False,
)
```

- **Run the export from a separate MCP call** than the one that built the mesh. `export_scene.gltf` needs a fresh `bpy.context` or it throws `Context has no attribute active_object`. Same reason `open_mainfile` must be its own call: it breaks `bpy.context` for the rest of that call.
- **Winding.** Bevy backface-culls; Blender's viewport does not, so flipped faces look solid in Blender but render inside-out in game. Build each solid piece as its own closed manifold and run `bmesh.ops.recalc_face_normals` on **that piece's faces only**, then join. A global Recalculate Outside mis-guesses at interpenetrations. (This same trick keeps the tree canopy single-sided in `build_tree.py`.)
- **Never `me.materials.clear()` then re-append.** Clearing silently resets every polygon's `material_index` to 0, so a later "recolour the head faces" loop matches nothing. Replace slots in place: `me.materials[0] = haft; me.materials[1] = head`.
- **Keep heads a flat-ish slab in depth.** The icon's chunkiness lives in the silhouette; a head as deep as it is wide reads as a featureless cube filling the first-person view. Match the existing iron-hatchet head's screen footprint, not the literal icon size.

## Parametric families

Each script mirrors a Rust constant as its parity point and exports COLOR_0 glbs. `build_tree.py` exports with `export_materials='NONE'`; `build_pieces.py`, `build_door.py`, and `build_deployables.py` export with `export_materials='EXPORT'`. Either way the in-game material is built or selected in Rust by `setup_scene` in `src/app/scene/assets.rs`, so the export flag value differs but the embedded material is ignored or replaced at load time.

| Script | assets/ output | Rust load site | Material kind | Mirrored constant |
|---|---|---|---|---|
| `art/ore/build_nodes.py` (image-to-3D family, see next section) | `assets/ore/<type>/stage_{0,1,2}.glb` (5 types x 3 stages) + `assets/textures/ore/<type>.png` | `ResourceVisualAssets` (ore stage meshes), five `<type>_node_material`s | `ToonMaterial` per type (baked albedo, COLOR_0 = white) | `src/app/systems/items/resource_nodes/stages.rs` (3 stages; ratios also in `STAGE_RATIOS`) |
| `art/trees/build_tree.py` | `assets/trees/<species>_<size>/model.glb` (mesh0 trunk, mesh1 foliage) | bark/foliage/dead `ToonMaterial`s | `ToonMaterial` (bark, foliage, dead-bark variants) | `src/app/scene/mesh/trees.rs` geometry constants + `tree_mesh_height` (`src/app/scene/components.rs`; pine 4.5/6.6/9.1, birch 3.6/5.3/7.15) |
| `art/building/build_pieces.py` | `assets/building/<piece>_<tier>.glb` (6 pieces x 3 tiers) | `building_meshes` + `building_materials` | textured `StandardMaterial` per tier | `crate::building::piece_local_boxes` (`src/building/collision.rs` - `piece_local_boxes`) |
| `art/building/build_door.py` | `assets/items/{hewn_log_door,iron_door}/model.glb` | `hewn_door_*` / `iron_door_*` | textured `StandardMaterial` (iron door: roughness 0.55, metallic 0.8) | door panel geometry |
| `art/deployables/build_deployables.py` | UV-baked `assets/items/{workbench_t1,crude_furnace,storage_box_*,tool_cupboard,torch}/model.glb` + workbench rebuild | `DeployableVisualAssets` meshes, `toon_{wood,stone,fabric}_material` | `ToonMaterial` (see deployables section) | `DeployableProfile.collider_half_width` in `src/items.rs` (workbench 0.55, furnace 0.50) |
| `art/held/build_held.py` (image-to-3D family, see next section) | `assets/items/<id>/model.glb` for the five gathering tools (1 prim + `socket_grip` node), the hammer, building plan, melee weapons, arrow, explosives, and `generic_held` (1 prim, NO socket) + `assets/textures/held/<id>.png` | `HeldMesh::visual()` `baked_tool` rows (`src/items/visual.rs`), `baked_tool_materials` in `assets.rs`, sockets via `HeldGripSockets` | per-item baked-albedo `ToonMaterial` + `ToonViewmodelMaterial` | The held reference frame + socket contract above. Dark-steel calibration survives from the old sickle work: forged steel wants ~0.05-0.09 linear in the viewmodel (the cel path multiplies albedo by a ~3-4x noon scene probe, so brighter silvers out); a TRELLIS bake that lands NEAR-ZERO instead needs the `ALBEDO_CURVE` floor lift or it renders as a silhouette hole |

The sleeping bag glb (`assets/items/sleeping_bag/model.glb`) ships through the same `prim_mesh` path and uses `toon_fabric_material`.

## Image-to-3D asset families (`art/ore/` is the template)

The ore nodes were re-authored 2026-07-19 through the RunPod generation lane
instead of a parametric builder. The process is deliberately reusable: to
rework another asset family, copy the `art/ore/` directory shape and swap the
prompts. Per-family directory contents:

- `prompts.json` - the prompt set (one shared backbone, per-variant labels
  kept identical across types so the picker compares like with like).
- `gen_candidates.py` / `patch_prompts.py` - generate reference candidates
  through the 2D lane (rembg RGBA cutouts; the mesh worker REQUIRES real
  alpha) and write `pick.html` for side-by-side picking. When a colour drifts,
  patch the SUBJECT clause first: a correction in the tail never beats the
  leading noun phrase.
- `selection.json` - the pick record (which candidate variant per type).
- `gen_meshes.py` - submits picked references to the RunPod TRELLIS.2 worker
  (`~/Desktop/dev/mesh-worker`, needs its `.venv` for the S3 download),
  renders turntables, writes `review.html`. Every completed job also leaves
  its glb on the network volume under `outputs/<job_id>.glb`, so a dead
  client loses nothing.
- `build_nodes.py` - raw glb to game format: voxel remesh 0.015 m (makes the
  non-manifold TRELLIS output collapsible at all) -> planar dissolve 12 deg ->
  collapse to the tri budget -> Smart UV -> Cycles selected-to-active
  DIFFUSE/COLOR bake of the AI texture onto the low-poly (BEFORE the world-fit
  transform; the bake matches surfaces in world space) -> per-type albedo PNG +
  lean stage glbs (UVs + white COLOR_0, no material). Per-family knobs at the
  top: world fit (up-axis fix + footprint), stage ratios, albedo curves (a
  near-black reference needs a gamma lift or it renders as a silhouette hole).
- `candidates/` - COMMITTED: only the picked references survive; unpicked
  variants are pruned at retirement time. This is the authoring record.
- `meshes/` - GITIGNORED: raw image-to-3D glbs (~50 MB each) and built
  outputs, all regenerable from the committed inputs.

`art/held/` is the second family through this lane (the five gathering tools,
2026-07-19) and adds the held-item specifics on top of the ore recipe:

- **One picked image serves as BOTH the inventory icon and the mesh
  reference** (unlike ore, where the deposit and the yielded item differ).
  `prompts.json` carries a single toony backbone per item; the picker exports
  `{"<item_id>": <variant>}`; `make_icon_masters.py` derives the icon side
  (auto-rotate/flip to the head-upper-left convention, largest-alpha-component
  keep, recenter) and `gen_meshes.py` feeds the same file to TRELLIS.
- **Prompt-side lessons**: describe the item as FLOATING in empty space,
  evenly lit; 'lying' phrasing makes Flux infer a ground plane and paint an
  OPAQUE shadow no ban or alpha-strip removes. 'As one connected piece'
  guards against disassembled tools. Composition cues beat bans: to keep a
  sickle's handle short, fill the frame with the BLADE, not the tool.
- **Held items use TRELLIS's NATIVE low-poly export, not local reduction**
  (final recipe after two A/B rounds in `preview.html`, the local three.js
  viewer served with `python3 -m http.server 8321` from `art/held/`):
  `gen_meshes.py` submits with `--decimation-target ~10000` and the worker's
  own mesh-aware simplifier returns ~10k tris with FULL silhouette fidelity
  plus its own baked 2048 texture and UVs. `build_held.py` keeps all of it
  as-is; its whole job is PCA canonicalization into the held reference frame
  (`auto_level`: haft = major axis up, thin normal to Y, wider-end-up,
  working edge to +X; TRELLIS output pose varies with the reference
  composition), the `socket_grip` empty (contract above), white COLOR_0,
  extracting the embedded baseColor PNG as the per-item albedo, and the lean
  export. `trim_ground_disc` stays as a per-item knob.
- **Why local reduction was abandoned** (keep these failure modes in mind if
  it is ever revived): the voxel remesh ERODES anything thinner than the
  voxel (slimmed pick arms at 8 mm); a 12 deg planar dissolve facets the
  whole tool before the budget applies, and even 4 deg + 6k + smooth shading
  lost thin-feature volume; and the Cycles DIFFUSE/COLOR rebake returns
  BLACK for metallic-flagged regions regardless of base colour (the TRELLIS
  material carries a metallic-roughness map; zeroing source metallic fixes
  it). Extracting the native texture sidesteps the bake entirely.
  `ALBEDO_CURVE` (gamma/gain + near-black floor lift) remains available on
  the extracted texture.
- **Batch 2 (everything else, 2026-07-19) ships SOCKETLESS in each
  predecessor's frame.** The hammer, building plan, four melee weapons,
  arrow, three explosives, and the new generic held bundle (`generic_held`,
  which replaced the procedural bag cuboid every mesh-less equipable shows
  in hand) are `baked_tool` rows like the tools, but deliberately carry NO
  `socket_grip`: their per-item carry poses in `held.rs` (mallet pull-in,
  upright sword guard, couched spear, silhouette bundles) are tuned against
  the OLD glbs' local frames, and the socket path would bypass all of that
  tuning. Instead `build_held.py` fits each rebuild into its predecessor's
  measured frame (`HELD_FIT` per item: `z_min`/`height` or `width`,
  `center: grip|full`, `invert_head` for narrow-tipped silhouettes,
  `align_limb_down` for big-head short-handle shapes PCA cannot solve,
  `socket=False`). `HeldMesh::uses_grip_socket()` (src/items/visual.rs) is
  the allowlist that keeps the engine from even scanning the socketless glbs.
  The placed charges bind the same per-item baked `ToonMaterial` as the held
  view (`charge_body_material` in `src/app/systems/deployables.rs`), and the
  projectile visuals reuse the held layer table unchanged. The three
  ANIMATABLE viewmodels (bow, crossbow, bandage) keep their authored
  multi-primitive glbs: a single-prim rebuild cannot carry their rig slots,
  so their batch-2 picks ship as icons only until a rigging path exists.

Family-agnostic tools live in `art/pipeline/`: `render_turntable.py` (glb ->
N-angle strip + tri stats; pass `color` as the 5th arg when judging albedo,
the default hot-key + AgX rig desaturates colours) and `retopo_experiment.py`
(dissolve-angle sweep diagnostic).

Retirement is part of the rework: when the new family lands in `assets/`, the
old build script, masters, concepts, and any now-orphaned textures are deleted
in the same commit (see the rework hygiene rule in `ART-PIPELINE-REWORK.md`,
moving into the docs map when that file dissolves).

All embedded PNG textures (tree bark/foliage, per-type ore albedos, deployable wood/stone/fabric, building/door tiers) are decoded synchronously and given a CPU mip chain via `build_mip_chain` (`src/app/scene/terrain.rs`) with a repeat + anisotropic sampler, because Bevy 0.18 builds no mips for loaded PNGs and the grain would otherwise alias into sparkle at range.

The build-script docstrings carry the full upstream chain (ComfyUI Flux concept -> OpenCV silhouette measurement -> Blender). Some of those docstrings predate the cel migration and still say "Rust builds the StandardMaterials"; the live `assets.rs` uses `ToonMaterial` for ore, trees, and deployables. Trust `assets.rs`, not the script header, for material kind.

## Deployables are cel-shaded with baked UVs

This is the section that drifted hardest in the old doc. The current reality:

- Deployable glbs carry `POSITION/NORMAL/COLOR_0` but originally had **no UVs**. `art/deployables/build_deployables.py` bakes box-projected `cube_uv` UVs into the furnace, both chests, the tool cupboard, and the torch, and **rebuilds the workbench from scratch** (plank bench, not the old 4-leg stool). Geometry of the unwrapped props is otherwise untouched.
- The surface is `detail_texture * COLOR_0`. The detail textures (`assets/textures/deployables/{wood,stone,fabric}.png`) are near-white plank/cobble line-art, so COLOR_0 carries the wood-brown / stone-grey and the texture multiplies dark seams on top.
- Rust attaches a `ToonMaterial`, **not** a base-white `StandardMaterial` and **not** a `glb_material` call. The per-prop selection is in `deployable_visual` in `src/app/systems/deployables.rs`:
  - furnace -> `toon_stone_material`
  - workbench, storage boxes, tool cupboard, torch -> `toon_wood_material`
  - sleeping bag -> `toon_fabric_material`
  - building pieces -> tier `StandardMaterial`; doors -> variant `StandardMaterial` (both still PBR)
- Deployable `ToonMaterial` params are `(3.0, 0.0, 1.0, 1.4)` with `tex_scale: 1.5`: a **punchier** cel than the softer rounded ore/trees, because the boxy props want every beveled corner to read as a drawn outline. The exact per-family params and the cel mechanics live in [toon-shading.md](../toon-shading.md). `tex_scale` is the triplanar fallback; it only matters if a prop ships without UVs, so keep the UV bake current.

If you author a new deployable: run it through `build_deployables.py` to bake UVs, add it to `DeployableVisualAssets`, and wire a `DeployableMaterial::Toon(...)` arm in `deployable_visual`. Do not reach for `glb_material`.

## Wire a model into Rust

In `src/app/scene/assets.rs` (`insert_deployable_visuals`, one of the `setup_scene` helpers), the DRY loader closure already exists:

```rust
let prim_mesh = |glb: &str, primitive: usize| asset_server.load(GltfAssetLabel::Primitive { mesh: 0, primitive }.from_asset(glb.to_owned()));
```

Glb paths come from `embedded_asset_path("items/<id>/model.glb")` (re-exported from `embedded_assets::asset_path` in `src/app.rs`). For a **held item**, add a row to the declarative `HeldMesh::visual()` table (`src/items/visual.rs`): an image-to-3D rebuild is one `baked_tool(glb, item_id)` row (single primitive, `HeldMeshMaterial::Baked` family; the per-item material pair is built automatically in `assets.rs` from `textures/held/<id>.png`, and its `socket_grip` resolves through `HeldGripSockets`); a shared-family glb lists its layers with their families. Held items render as overlaid layers sharing one swing transform; the same table feeds the third-person rig and the paperdoll so peers see what is held. For a **parametric prop**, load only `prim_mesh(&glb, 0)` and attach the shared material (no `glb_material`).

`build.rs` fingerprints the `assets/` tree, so a re-exported glb re-embeds on the next `cargo build`. Remove Blender's `.blend1` auto-backup before finishing.

## Icons: the skill<->repo boundary

This is the single biggest mental model to get right. **Icon generation lives in the `lowpoly-game-assets` skill, not the repo.** The repo holds only the downscale finalizer.

- `gen_icon_ref.py` (img2img reskin from a reference icon) and `generate.py` (text-to-icon, tileable textures) are in `~/.claude/skills/lowpoly-game-assets/scripts/`. Do not grep the repo for them.
- `scripts/icon_finalize.py` is the committed repo script. It does only the `art/items/<id>/icon_master_512.png` -> `assets/items/<id>/icon.png` downscale (default 160px). Rationale, from its docstring: egui user textures have **no mipmaps**, so each icon minifies into its slot with plain bilinear. It uses ImageMagick's **Triangle** filter (no negative lobes, unlike Lanczos which rings into bright edge speckles) and **edge-bleeds** opaque RGB outward first so straight-alpha bilinear never interpolates undefined RGB across the silhouette. Both transforms are always-on and safe; `--desaturate`, `--smooth`, `--despeckle` are opt-in (never `--desaturate` a colourful icon like an ore or ember).

`icon_finalize.py` itself defers img2img to the skill's `gen_icon_ref.py`. Every `art/items/<id>/` dir carries `icon_master_512.png`; only those with a `.blend` are hand-modelled 3D sources (hammer, building_plan, crude_furnace, workbench_t1, both storage boxes, tool_cupboard, torch). The five gathering tools' mesh sources live in `art/held/` (picked candidates + prompts + selection are the committed authoring record); everything else is a flat inventory icon with no model.

### Deriving a sibling tool

When a new item shares an existing tool's shape (a stone tier of an iron tool, a reskin):

- **Image-to-3D families**: duplicate the item's variant block in the family's `prompts.json`, swap the material clause in the SUBJECT noun phrase (a correction in the tail never beats the leading noun phrase), and run the same candidates -> pick -> mesh -> build loop. Like-numbered icon and reference variants describe the same design, so the pair stays coherent.
- **Hand-modelled glbs**: icon via img2img from the sibling's icon (`gen_icon_ref.py --ref <sibling>/icon_master_512.png --subject "<new material> <tool>"`, `--strength` ~0.85 for a big material change), then copy the sibling's `.blend` geometry, swap the head material, recolour the head loops, export. A whole tier family can share one geometry and differ only in the head material.

## Verify in-game

Build (`cargo build` re-embeds the glb), launch the headless harness ([headless-agent-testing.md](../headless-agent-testing.md)), and screenshot. First-person reveals problems a Blender render hides.

- **Held tool:** grant a debug kit with `/test-kit` (alias `/testkit`, admin only, `command_test_kit` in `src/server/commands/kit.rs`). The actionbar fills equipables first in `EQUIPABLES` order: stone hatchet (0), stone pickaxe (1), iron hatchet (2), iron pickaxe (3), then workbench, furnace, building plan, hammer, hewn door, sleeping bag.
- **Deployable:** `scripts/ashwend-control.py <sock> place-deployable <item_id> [distance]` drops the carried structure in front of the player, facing them (position from view yaw, so it works without aiming at the ground). The control socket lives in `src/control_socket/`.

## Related docs

- [docs/toon-shading.md](../toon-shading.md): the cel `ToonMaterial` shared by ore, trees, and deployables; per-family `params`, band/edge tuning, the `fade` uniform.
- [docs/rendering-materials.md](../rendering-materials.md): StandardMaterial PBR conventions, the linear COLOR_0 palette calibration, and which families are StandardMaterial vs ToonMaterial.
- [docs/items-and-resources.md](../items-and-resources.md): how `held_mesh` and item ids map to the renderer.
- [docs/playbooks/add-content.md](add-content.md): the surrounding "add a tool / ore / deployable" steps this art work plugs into.
- [docs/headless-agent-testing.md](../headless-agent-testing.md): the control socket + capture harness used to verify a model in-game.
