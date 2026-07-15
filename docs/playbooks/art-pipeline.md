---
title: "Playbook: author a model or icon"
owns: The repeatable model/icon authoring pipeline (hand-modelled held tools, parametric-script glb families, and the skill<->repo boundary).
when_to_read: When modelling a new held item or prop, deriving a sibling tool, authoring a deployable/building glb, or generating an icon/texture.
sources:
  - "src/app/scene/assets.rs - setup_scene asset wiring (the prim_mesh closure in insert_deployable_visuals, ItemVisualAssets, DeployableVisualAssets)"
  - "src/app/systems/items/held.rs - held_item_layers"
  - "src/app/scene/toon.rs - ToonMaterial"
  - "art/ore/build_ore.py - parametric ore glbs"
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

Two facts shape everything below: in-game props get their colour from a `COLOR_0` vertex-colour attribute baked into the glb (LINEAR albedos, see [rendering-materials.md](../rendering-materials.md)), and the **material is attached in Rust at load time**, not exported from the model, for every family except the four held tools. The icon-generation half (img2img reskins, tileable texture synthesis) lives in the `lowpoly-game-assets` skill, not the repo; the repo owns the build scripts, `icon_finalize.py`, and the `assets.rs` wiring.

## Two authoring tracks

| | Hand-modelled held tools | Parametric headless scripts |
|---|---|---|
| What | The four tools: stone/iron pickaxe + hatchet | ore nodes, trees, building pieces, doors, deployables |
| How | Blender driven live over the Blender MCP; the `.blend` is source of truth | `art/<family>/build_*.py` run headless (`Blender --background --python ...`) |
| `export_materials` | `'EXPORT'` (glb embeds its own materials) | ore + trees use `'NONE'`; building, door, and deployable scripts use `'EXPORT'` (Rust attaches/selects the shared material either way) |
| Primitives | 2 (matte haft = prim 0, worked head = prim 1) | typically 1 (trees = 2: trunk + foliage) |
| Material kind | `StandardMaterial`, two per tool, tinted by COLOR_0 | mostly `ToonMaterial`; building pieces + doors are textured `StandardMaterial` |
| Rust load | `prim_mesh` (closure in `assets.rs` - `insert_deployable_visuals`) | `prim_mesh` only; material built/selected in Rust |

Rule of thumb: **do not** commit a headless build script for a hand-modelled tool, and **do not** hand-model in interactive Blender a shape that must stay in lockstep with a Rust constant (a building piece, a tree silhouette, an ore stage). Those go through a parametric script that mirrors the constant.

Note the deployables straddle the line: their `.blend` source was hand-authored, but `art/deployables/build_deployables.py` post-processes them (bakes UVs, rebuilds the workbench). Treat them as the parametric track for material/UV purposes.

The Blender MCP (`mcp__Blender__execute_blender_code`, render helpers) is an **external dependency** that may not be connected in a given session. The hand-modelled track requires it; the parametric scripts only need a local Blender binary.

## Held-item reference frame

Every held tool is scaled into one shared reference frame so the existing swing transform and poses just work. Read the reference glb bounds first (`assets/items/iron_hatchet/model.glb`), do not invent new anchors:

- prim 0 (haft) POSITION min Y = `-0.514` (the pommel)
- prim 1 (head) POSITION max Y = `0.356` (head top, ~0.36)
- total height ~`0.87`

Build in the authoring frame (Blender is Z-up, the glb exports +Y up via `export_yup=True`): handle along Blender `+Z` (in-game up), head blade/prong axis along Blender `+X` (in-game forward, so the working edge faces the swing), thin axis along Blender `+Y`. Solve eye height and handle length from the two anchors, then place the icon-measured silhouette (in handle-length units) into that frame.

**Measure the icon, do not eyeball it.** Extract the silhouette with OpenCV (numpy is installed; PIL is not) and transform it into the authoring frame: classify pixels (warm `R - B` = wood, low-saturation grey = stone/iron), find the handle axis by PCA anchored on the unambiguous class, locate the eye (handle end nearest the head) and pommel (farthest), then map the head contour into a handle-aligned, eye-origin frame scaled so handle length = 1.0. The measure script is scratch (`/tmp`, not committed).

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

Each script mirrors a Rust constant as its parity point and exports COLOR_0 glbs. `build_ore.py` and `build_tree.py` export with `export_materials='NONE'`; `build_pieces.py`, `build_door.py`, and `build_deployables.py` export with `export_materials='EXPORT'`. Either way the in-game material is built or selected in Rust by `setup_scene` in `src/app/scene/assets.rs`, so the export flag value differs but the embedded material is ignored or replaced at load time.

| Script | assets/ output | Rust load site | Material kind | Mirrored constant |
|---|---|---|---|---|
| `art/ore/build_ore.py` | `assets/ore/<type>/stage_{0,1,2}.glb` (4 types x 3 stages) | `ResourceVisualAssets` (ore stage meshes), `ore_toon_material` | `ToonMaterial` (one shared, COLOR_0 = per-mineral) | `src/app/systems/items/resource_nodes/stages.rs` (3 stages) |
| `art/trees/build_tree.py` | `assets/trees/<species>_<size>/model.glb` (mesh0 trunk, mesh1 foliage) | bark/foliage/dead `ToonMaterial`s | `ToonMaterial` (bark, foliage, dead-bark variants) | `src/app/scene/mesh/trees.rs` geometry constants + `tree_mesh_height` (`src/app/scene/components.rs`; pine 4.5/6.6/9.1, birch 3.6/5.3/7.15) |
| `art/building/build_pieces.py` | `assets/building/<piece>_<tier>.glb` (6 pieces x 3 tiers) | `building_meshes` + `building_materials` | textured `StandardMaterial` per tier | `crate::building::piece_local_boxes` (`src/building/collision.rs` - `piece_local_boxes`) |
| `art/building/build_door.py` | `assets/items/{hewn_log_door,iron_door}/model.glb` | `hewn_door_*` / `iron_door_*` | textured `StandardMaterial` (iron door: roughness 0.55, metallic 0.8) | door panel geometry |
| `art/deployables/build_deployables.py` | UV-baked `assets/items/{workbench_t1,crude_furnace,storage_box_*,tool_cupboard,torch}/model.glb` + workbench rebuild | `DeployableVisualAssets` meshes, `toon_{wood,stone,fabric}_material` | `ToonMaterial` (see deployables section) | `DeployableProfile.collider_half_width` in `src/items.rs` (workbench 0.55, furnace 0.50) |

The sleeping bag glb (`assets/items/sleeping_bag/model.glb`) ships through the same `prim_mesh` path and uses `toon_fabric_material`.

All embedded PNG textures (tree bark/foliage, ore rock, deployable wood/stone/fabric, building/door tiers) are decoded synchronously and given a CPU mip chain via `build_mip_chain` (`src/app/scene/terrain.rs`) with a repeat + anisotropic sampler, because Bevy 0.18 builds no mips for loaded PNGs and the grain would otherwise alias into sparkle at range.

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

Glb paths come from `embedded_asset_path("items/<id>/model.glb")` (re-exported from `embedded_assets::asset_path` in `src/app.rs`). For a **held tool**, store `*_body_mesh` / `*_head_mesh` (prims 0/1) and `*_body_material` / `*_head_material` (materials 0/1) in `ItemVisualAssets`, then add the two layers to `held_item_layers` in `src/app/systems/items/held.rs`. Held items render as overlaid layers sharing one swing transform, one layer per primitive; this same function feeds the third-person rig so peers see what is held. For a **parametric prop**, load only `prim_mesh(&glb, 0)` and attach the shared material (no `glb_material`).

`build.rs` fingerprints the `assets/` tree, so a re-exported glb re-embeds on the next `cargo build`. Remove Blender's `.blend1` auto-backup before finishing.

## Icons: the skill<->repo boundary

This is the single biggest mental model to get right. **Icon generation lives in the `lowpoly-game-assets` skill, not the repo.** The repo holds only the downscale finalizer.

- `gen_icon_ref.py` (img2img reskin from a reference icon) and `generate.py` (text-to-icon, tileable textures) are in `~/.claude/skills/lowpoly-game-assets/scripts/`. Do not grep the repo for them.
- `scripts/icon_finalize.py` is the committed repo script. It does only the `art/items/<id>/icon_master_512.png` -> `assets/items/<id>/icon.png` downscale (default 160px). Rationale, from its docstring: egui user textures have **no mipmaps**, so each icon minifies into its slot with plain bilinear. It uses ImageMagick's **Triangle** filter (no negative lobes, unlike Lanczos which rings into bright edge speckles) and **edge-bleeds** opaque RGB outward first so straight-alpha bilinear never interpolates undefined RGB across the silhouette. Both transforms are always-on and safe; `--desaturate`, `--smooth`, `--despeckle` are opt-in (never `--desaturate` a colourful icon like an ore or ember).

`icon_finalize.py` itself defers img2img to the skill's `gen_icon_ref.py`. Of the 20 `art/items/<id>/` dirs, all 20 carry `icon_master_512.png`; only the 10 with a `.blend` have a 3D model (the four tools + crude_furnace, workbench_t1, both storage boxes, tool_cupboard, torch). The other 10 are flat inventory icons with no model.

### Deriving a sibling tool

When a new item shares an existing tool's shape (a stone tier of an iron tool, a reskin), do not remodel from scratch:

1. **Icon via img2img** from the sibling's icon using the skill: `gen_icon_ref.py --ref <sibling>/icon_master_512.png --subject "<new material> <tool>"`. Push `--strength` up (~0.85) if the material changes a lot (iron head -> rough stone). Run the OpenCV measure on the result to confirm it carries the sibling's silhouette.
2. **Model by copying the sibling's `.blend` geometry** (`me = sibling_obj.data.copy()`), swap the head material (matte stone: metallic 0, roughness 0.9), recolour the head loops into a stone palette, add bindings (fibre lashing = a few twine-coloured ring bands at the joint), export. The copied footprint is already first-person-comfortable, so no rescaling. A whole tier family can share one geometry and differ only in the head material.

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
