# Icon and concept to 3D model

How to turn a painted item icon (or concept art) into an authored 3D model that
matches it, repeatably. This is the pipeline the four tools (stone and iron
pickaxe and hatchet) were built with. Read this before modelling a new held
item or any prop that should match a 2D reference closely, so the workflow does
not have to be rediscovered each time.

## When to use this (vs procedural meshes)

- **Authored glb (this doc):** held-item viewmodels and props that should match
  a painted icon or concept closely, especially organic or faceted shapes that
  boxes and prisms read poorly (a curved double pick, a chunky faceted maul, a
  star-shaped stone head). The model carries its own geometry **and** materials.
- **Procedural `LowPolyMeshBuilder`** (`src/app/scene/mesh/`): still the right
  tool for algorithmic or blocky shapes (ore nodes, trees, debris, deployable
  structures). Do not author those in Blender, and do not reshape a procedural
  mesh to chase an icon, author a glb instead.

## Tools

- **Blender**, driven live over the **Blender MCP** (`mcp__Blender__execute_blender_code`,
  plus `render_viewport_to_path` / a render call for previews). Edit the live
  session directly. Do not commit a headless Python build script; the `.blend`
  is the source of truth. Notes that bite every time:
  - `bpy` is **not** auto-imported in the exec scope. Start with `import bpy`.
  - The value you assign to `result` **must be a dict**.
  - `get_screenshot_of_*` sometimes errors; fall back to `bpy.ops.render.render`
    to a `/tmp` PNG and `Read` it.
  - Blender 5.x render engine enum is `'BLENDER_EEVEE'` (not `_NEXT`).
  - `read_homefile(use_empty=False)` then delete the default cube gives a clean
    scene with a camera and light for previews, without touching any `.blend` on
    disk.
- **OpenCV + numpy** (both installed; **PIL is not**) to measure the icon
  silhouette. This is the step that turns "still not right" into a one-pass
  match; see below.
- **ImageMagick** (`magick`) to crop and enlarge capture screenshots for review.
- **The headless capture harness** to verify the model in-game (control socket +
  `GAME_HEADLESS_CAPTURE`). See [Multiplayer testing](multiplayer-testing.md).

## File layout (source of truth)

| File | Role |
|---|---|
| `art/items/<id>/icon_master_512.png` | the painted reference (committed) |
| `art/items/<id>/<id>.blend` | editable Blender source, edited directly (committed) |
| `assets/items/<id>/model.glb` | exported runtime model, embedded by `build.rs` |
| `assets/items/<id>/icon.png` | the downscaled in-game inventory icon |

`build.rs` fingerprints `assets/`, so a re-exported glb re-embeds on the next
`cargo build`. Remove Blender's `.blend1` auto-backup before finishing.

## Pipeline

### 1. Measure the icon, do not eyeball it

Eyeballing proportions fails repeatedly. Instead extract the silhouette with
OpenCV and transform it into the model's authoring frame:

1. Load the icon RGBA. Foreground = `alpha > 128`.
2. Classify pixels: **wood** is warm (`R - B` above a threshold); **stone/iron**
   is low-saturation grey; everything else is background. (Grey covers both stone
   and iron heads, the same classifier works for both.)
3. Find the **handle axis** by PCA. Anchor on whichever class is unambiguous: for
   a tool with leather straps that get misread as wood, anchor on the grey stone
   head, dilate it into a "head region", and take the handle from wood pixels
   **outside** that region so the straps do not bias the axis.
4. Locate the **eye** (head and handle junction = handle end nearest the head)
   and the **pommel** (handle end farthest from the head). Handle length is
   eye to pommel.
5. Transform the head contour (`cv2.approxPolyDP` of the largest grey component,
   hard-closed to bridge straps) into a **handle-aligned, eye-origin frame
   scaled so handle length = 1.0**. Now the silhouette is in tool-relative units.
6. Draw the result back onto the icon and `Read` it to sanity-check before
   building anything.

The measurement script is scratch (write it to `/tmp`, it is not committed).

### 2. Author the model in Blender (via the MCP)

Build in the **authoring frame** (Blender is Z-up; the glb exports +Y up):

- Handle along Blender **+Z** (becomes in-game up).
- Head blade / prong axis along Blender **+X** (in-game forward, so a blade or
  pick faces the swing).
- Thin axis along Blender **+Y**.

Scale to the **shared held-item reference frame** so the existing swing
transform and poses just work. Read the reference glb bounds first
(`assets/items/iron_hatchet/model.glb`): pommel at glb Y `-0.514`, head top
`~0.36`, total height `~0.87`. Solve the eye height and handle length from those
two anchors, then place the icon-measured silhouette (in handle-length units)
into that frame.

Two material slots become two glb primitives:

- **Primitive 0 = matte haft** (the wood handle). Fold the leather straps or
  twine wraps into this same material as **extra vertex colours**, so a binding
  does not need a third material slot.
- **Primitive 1 = the worked head** (faceted stone, or shiny iron).

Per-face tone comes from a `COLOR_0` vertex-colour attribute, using the **linear**
palette in `src/app/scene/mesh/builder.rs` so authored models match procedural
ones.

Material setup (each slot needs a Vertex Color node wired to Base Color, or
`COLOR_0` exports white):

```python
m = bpy.data.materials.new(name); m.use_nodes = True
nt = m.node_tree
bsdf = nt.nodes["Principled BSDF"]      # or recreate the node
bsdf.inputs["Roughness"].default_value = 0.92
bsdf.inputs["Metallic"].default_value = 0.0
bsdf.inputs["Specular IOR Level"].default_value = 0.15   # -> Bevy reflectance
vc = nt.nodes.new("ShaderNodeVertexColor"); vc.layer_name = "Color"
nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
m.use_backface_culling = True            # mirror Bevy so flipped faces show in preview
```

Use a corner FLOAT_COLOR attribute (`bm.loops.layers.float_color.new("Color")`),
keep faces flat-shaded (`use_smooth = False`), and set "Color" as the active
**render** colour (`mesh.color_attributes.render_color_index`).

Render a front and a three-quarter preview after every change and `Read` them.

### 3. Export the glb

Select only the object, set it active, then export. **Run the export from a
separate MCP call** than the one that built the mesh; a fresh `bpy.context` is
needed or `export_scene.gltf` throws `Context has no attribute active_object`
(the `.blend` save in the build call still succeeds).

```python
bpy.ops.export_scene.gltf(
    filepath="assets/items/<id>/model.glb",
    export_format='GLB', use_selection=True, export_yup=True,
    export_apply=True, export_normals=True,
    export_materials='EXPORT', export_texcoords=False,
)
```

Verify the glb before trusting it: parse it and confirm two primitives, a
non-white `COLOR_0` per primitive, and POSITION bounds matching the reference
frame (pommel `-0.514`, head top `~0.36`).

### 4. Wire it into Rust

In `src/app/scene/assets.rs`, load each glb's primitive meshes and materials
with the DRY closures already there:

```rust
let glb = embedded_asset_path("items/<id>/model.glb");
let prim_mesh   = |glb, p| asset_server.load(GltfAssetLabel::Primitive { mesh: 0, primitive: p }.from_asset(glb));
let glb_material = |glb, i| asset_server.load(GltfAssetLabel::Material { index: i, is_scale_inverted: false }.from_asset(glb));
```

Store a `*_body_mesh` / `*_head_mesh` and `*_body_material` / `*_head_material`
per tool in `ItemVisualAssets`, and return both layers from `held_item_layers`
in `src/app/systems/items/held.rs`. (Held items render as overlaid layers
sharing one swing transform; one layer per primitive.)

### 5. Verify in-game

Build (`cargo build` re-embeds the glb), run the headless harness, put the item
in hand, and screenshot. First-person reveals problems a Blender render hides
(for example, a head as deep as it is wide reads as a featureless cube filling
the view). See [Multiplayer testing](multiplayer-testing.md) for the launch
recipe; after `/test-kit` the actionbar order is stone hatchet 0, stone pickaxe
1, iron hatchet 2, iron pickaxe 3.

## Gotchas that cost a debug cycle

- **Outward winding (backface culling).** Bevy backface-culls; Blender's viewport
  does not, so flipped faces look solid in Blender but render inside-out in game.
  Build each solid piece (handle lathe, head prism, each strap box) as its own
  closed manifold and run `bmesh.ops.recalc_face_normals` on **that piece's faces
  only**, then join. A global `Recalculate Outside` mis-guesses where the handle
  interpenetrates the head. Confirm with a `use_backface_culling = True` render
  (no black holes) or the Face Orientation overlay.
- **`COLOR_0` exports white** unless the "Color" attribute is the active *render*
  colour **and** each material feeds it through a Vertex Color node into Base
  Color.
- **Taper blade and prong thickness by distance to the cutting curve**, not by
  per-vertex flags. Compute each silhouette vertex's thickness from its distance
  to the whole cutting-edge or nearest-tip polyline. Flagging only some edge
  verts thin leaves an abrupt thin-to-thick "fin" at the top of the blade.
- **Keep heads a flat-ish slab in depth.** The icon's chunkiness lives in the
  silhouette, not the depth; a deep block reads as a cube in first person.
- **Size the head for first person, not the literal icon.** An icon can show a
  head almost as large as the handle; reproduced at that scale it fills the
  screen in-hand. Match the established tool's screen footprint instead (the
  iron hatchet head is the reference), then verify in-game.
- **Read the shape as the tool it is, not just a silhouette to extrude.** A
  pickaxe is a double-pointed pick: a central socket where the handle enters and
  two sharp prongs sweeping out (a wide crescent). Extruding the raw contour as a
  flat star plate reads as "an odd rock", not a pick. Give it clean sharp prongs;
  the forward one is the working point for downward node slams.
- **Handedness is free.** The icon is a 2D display image, so the model's left
  and right need not match it. Only the silhouette and proportions do.
- **Swapping materials: never `me.materials.clear()` then re-append.** Clearing
  the slots silently resets every polygon's `material_index` to 0, so a later
  "recolour only the head faces (index 1)" loop matches nothing and the head
  keeps its old colours. Replace in place: `me.materials[0] = haft;
  me.materials[1] = stone`.
- **`open_mainfile` breaks `bpy.context` for the rest of that MCP call.** Open in
  one call, build/export in the next. Inside a call, prefer the data API
  (`src.data.copy()`, `bpy.data.objects.new`) over `bpy.ops.object.duplicate`.

## Deriving a sibling tool (same shape, different material)

When a new item should share an existing tool's shape (a stone tier of an iron
tool, a reskin), do NOT remodel it from scratch. Two moves, both keep them
perfectly consistent:

1. **Icon via img2img from the sibling's icon.** `gen_icon_ref.py --ref
   <sibling>/icon_master_512.png --subject "<new material> <tool>"` keeps the
   silhouette and pose, swaps the surface. Push `--strength` up (~0.85) if the
   material needs to change a lot (iron head -> rough stone); too low keeps the
   old look. This is how the iron tools were first made from the stone ones, and
   later how the stone tools were redone to match the iron ones (stone heads +
   fibre lashing). Run the OpenCV measure on the result to confirm it carries the
   sibling's silhouette.
2. **Model by copying the sibling's `.blend` geometry**, then recolour and add
   detail: `me = iron_obj.data.copy()`, swap the head material to the new one
   (e.g. matte stone: metallic 0, roughness 0.9), recolour the head loops by a
   face-normal lambert into a stone palette, and add any extra bindings (fibre
   lashing = a few twine-coloured 8-gon ring bands around the handle at the
   joint). Export. The result matches the sibling's footprint exactly, so it is
   already first-person-comfortable and needs no rescaling.

This beats re-extruding a measured contour for sibling tools: it is more robust
(no contour noise) and guarantees the family reads as one set.

Taken to its conclusion, a whole tier family can **share one geometry** and let
only the head material distinguish tiers: the stone and iron tools use identical
meshes (same shape, same fibre lashing) and differ purely in the head, matte
grey stone (`metallic 0, rough 0.9`) versus metallic steel (`metallic 1, rough
0.34`). Each tier is still its own glb so they can diverge later, but keeping the
geometry identical is the cheapest way to make a progression read as one family.

## See also

- [Items and resources](items-and-resources.md): the "Editing a tool model"
  section and how `held_mesh` / `ItemModel` map to the renderer.
- [Materials](materials.md): PBR reflectance / roughness conventions.
- [Multiplayer testing](multiplayer-testing.md): the headless capture harness.
- Memory `blender-glb-item-models`: the running log of decisions and fixes.
