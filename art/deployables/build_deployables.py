"""Deployable model post-processing + the workbench rebuild.

The deployable props (workbench, crude furnace, storage chests, tool cupboard,
torch) were authored as vertex-colour glbs with POSITION/NORMAL/COLOR_0 but NO
UVs. Once the world moved to the cel-shaded `ToonMaterial` the props needed a
real surface texture (plank / cobble line-art), and with no UVs the shader fell
back to world-space triplanar projection: the plank courses ran whatever
direction each face happened to point and drifted as the prop moved. The fix is
to bake proper UVs into the meshes.

This script does two things, mirroring `art/building/build_pieces.py`:

  1. UNWRAP the five accepted models (furnace, both chests, cupboard, torch):
     import the committed glb, add a `cube_uv` box-projection UV layer (the same
     per-face projection the building pieces use, so vertical faces get
     horizontal plank courses), keep COLOR_0, and re-export the glb with
     TEXCOORD_0. Geometry is untouched, so the silhouettes stay exactly as
     authored.

  2. REBUILD the workbench from scratch: the old one read as a plain 4-leg
     stool. This builds a sturdy plank bench (thick plank top, chunky legs,
     aprons, a lower shelf) with the same box-projected UVs and COLOR_0. Kept
     all-wood: earlier metal vise + loose tools read as untextured and the vise
     z-fought against the lip, so they were dropped.

The surface colour is `detail_texture * COLOR_0`. The deployable detail textures
(`assets/textures/deployables/{wood,stone}.png`) are near-white line-art (plank
seams / cobble outlines), so COLOR_0 carries the wood-brown / stone-grey and the
texture multiplies the dark detail on top. Rust binds the wood texture to the
wooden props and the stone texture to the furnace (see
`DeployableVisualAssets` in src/app/scene/assets.rs).

Run headless:
  /Applications/Blender.app/Contents/MacOS/Blender --background \
      --python art/deployables/build_deployables.py
"""

import bpy
import bmesh
import math
import os

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ITEMS = os.path.join(REPO, "assets", "items")

# Texture repeat (metres per tile). Smaller = finer planks / cobbles / weave.
TILE_WOOD = 0.70
TILE_STONE = 0.95
TILE_FABRIC = 0.42


# ----------------------------------------------------------------- UV projection
def cube_uv(co, n, tile):
    """Per-face box projection in Blender Z-up space. The dominant normal axis
    selects the projection plane; the second returned coord (v) is kept as world
    "up" (Z) on the vertical faces so the plank line-art runs as horizontal
    courses wrapping the prop, matching `art/building/build_pieces.py`."""
    ax, ay, az = abs(n[0]), abs(n[1]), abs(n[2])
    if az >= ax and az >= ay:        # top / bottom (normal points up)
        u, v = co[0], co[1]
    elif ax >= ay and ax >= az:      # +/-X faces
        u, v = co[1], co[2]          # v = Z (up)
    else:                            # +/-Y faces
        u, v = co[0], co[2]          # v = Z (up)
    return (u / tile, v / tile)


def add_box_uvs(me, tile):
    """Add (or replace) a UVMap on a finished mesh via `cube_uv`, non-destructive
    to geometry. Leaves COLOR_0 as the render/active colour so export re-emits
    it."""
    for layer in list(me.uv_layers):
        me.uv_layers.remove(layer)
    uvl = me.uv_layers.new(name="UVMap")
    for poly in me.polygons:
        n = poly.normal
        for li in poly.loop_indices:
            co = me.vertices[me.loops[li].vertex_index].co
            uvl.data[li].uv = cube_uv(co, n, tile)
    if me.color_attributes:
        i = 0
        me.color_attributes.render_color_index = i
        me.color_attributes.active_color_index = i


def ensure_vertex_color_material(obj):
    """Wire the object's material so its Base Color reads the COLOR_0 attribute,
    guaranteeing the gltf exporter emits COLOR_0 (same trick build_pieces uses)."""
    color_name = obj.data.color_attributes[0].name if obj.data.color_attributes else "Color"
    m = obj.data.materials[0] if obj.data.materials else bpy.data.materials.new(obj.name + "_mat")
    if not obj.data.materials:
        obj.data.materials.append(m)
    m.use_nodes = True
    nt = m.node_tree
    bsdf = nt.nodes.get("Principled BSDF") or next(
        (nd for nd in nt.nodes if nd.type == "BSDF_PRINCIPLED"), None
    )
    if bsdf is None:
        return
    vc = next((nd for nd in nt.nodes if nd.type == "VERTEX_COLOR"), None)
    if vc is None:
        vc = nt.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = color_name
    nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])


def export_glb(obj, out_path):
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=out_path, export_format="GLB", use_selection=True,
        export_yup=True, export_apply=True, export_normals=True,
        export_materials="EXPORT", export_texcoords=True,
    )


# --------------------------------------------------------------------- unwrap path
def unwrap_glb(rel_glb, tile, bevel=0.0):
    """Round-trip a committed deployable glb, adding box-projected UVs. When
    `bevel` > 0 the box edges are rounded first (segments=2) so the cel
    ink-edge inks every corner and the bands catch the rounded facets, giving
    the boxy props a hand-drawn read instead of flat-shaded planes. Bevel
    interpolates COLOR_0 onto the new facets; UVs are re-projected afterwards."""
    path = os.path.join(ITEMS, rel_glb)
    bpy.ops.wm.read_homefile(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=path)
    meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    if len(meshes) != 1:
        raise RuntimeError(f"{rel_glb}: expected 1 mesh, got {len(meshes)}")
    obj = meshes[0]
    # The gltf importer parents the mesh under the scene root and may carry a
    # node transform; bake it so the exported geometry is centred exactly like
    # the source asset.
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    if bevel > 0.0:
        bm = bmesh.new()
        bm.from_mesh(obj.data)
        bmesh.ops.bevel(bm, geom=list(bm.edges), offset=bevel, segments=2,
                        affect="EDGES", clamp_overlap=True)
        bm.normal_update()
        bm.to_mesh(obj.data)
        bm.free()
    add_box_uvs(obj.data, tile)
    ensure_vertex_color_material(obj)
    export_glb(obj, path)
    print("unwrapped", rel_glb, "bevel" if bevel else "")


# ------------------------------------------------------------------- bedroll build
BAG_FABRIC = (0.17, 0.30, 0.15)   # woven plant-fibre green (* the pale weave tex)
BAG_FABRIC_DK = (0.12, 0.22, 0.11)
BAG_LINING = (0.36, 0.29, 0.18)   # tan folded-back lining + pillow
BAG_LINING_LT = (0.42, 0.34, 0.21)


def build_sleeping_bag():
    """A bedroll laid along +X: padded roll, tapered foot, a folded-back lining
    at the head and a pillow bump. Mirrors the retired `sleeping_bag_mesh`
    layout but ships UVs + a bevel so the woven-fabric toon texture maps and the
    cloth edges read soft. Footprint sits inside the 1.6 m square collider."""
    bpy.ops.wm.read_homefile(use_empty=True)
    mesh = bpy.data.meshes.new("sleeping_bag")
    obj = bpy.data.objects.new("sleeping_bag", mesh)
    bpy.context.collection.objects.link(obj)
    bm = bmesh.new()
    col = bm.loops.layers.float_color.new("Color")
    uv = bm.loops.layers.uv.new("UVMap")

    def box(center, half, color):
        add_box(bm, col, uv, center, half, color, TILE_FABRIC)

    box((0.10, 0.10, 0.0), (0.85, 0.10, 0.40), BAG_FABRIC)       # main roll
    box((-0.65, 0.089, 0.0), (0.32, 0.085, 0.34), BAG_FABRIC_DK)  # tapered foot
    box((0.62, 0.19, 0.0), (0.30, 0.035, 0.36), BAG_LINING)       # folded lining
    box((0.78, 0.225, 0.0), (0.18, 0.05, 0.22), BAG_LINING_LT)    # pillow

    bmesh.ops.recalc_face_normals(bm, faces=list(bm.faces))
    bm.normal_update()
    bmesh.ops.bevel(bm, geom=list(bm.edges), offset=0.018, segments=2,
                    affect="EDGES", clamp_overlap=True)
    bm.normal_update()
    for f in bm.faces:
        f.normal_update()
        for loop in f.loops:
            loop[uv].uv = cube_uv(loop.vert.co, f.normal, TILE_FABRIC)
    bm.to_mesh(mesh)
    bm.free()
    if "Color" in mesh.color_attributes:
        i = mesh.color_attributes.find("Color")
        mesh.color_attributes.render_color_index = i
        mesh.color_attributes.active_color_index = i
    m = bpy.data.materials.new("sleeping_bag_mat")
    m.use_nodes = True
    bsdf = m.node_tree.nodes.get("Principled BSDF")
    vc = m.node_tree.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    m.node_tree.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    mesh.materials.append(m)
    out = os.path.join(ITEMS, "sleeping_bag", "model.glb")
    export_glb(obj, out)
    print("built sleeping bag")


# ------------------------------------------------------------------ workbench build
WOOD = (0.46, 0.30, 0.16)        # warm oak (linear; * the white plank texture)
WOOD_DK = (0.30, 0.19, 0.10)     # shadowed underside / shelf


def add_box_blender(bm, col, uv, lo, hi, color, tile):
    x0, y0, z0 = lo
    x1, y1, z1 = hi
    verts = [
        bm.verts.new((x0, y0, z0)), bm.verts.new((x1, y0, z0)),
        bm.verts.new((x1, y1, z0)), bm.verts.new((x0, y1, z0)),
        bm.verts.new((x0, y0, z1)), bm.verts.new((x1, y0, z1)),
        bm.verts.new((x1, y1, z1)), bm.verts.new((x0, y1, z1)),
    ]
    faces = [(0, 1, 2, 3), (4, 7, 6, 5), (0, 4, 5, 1),
             (2, 6, 7, 3), (1, 5, 6, 2), (0, 3, 7, 4)]
    for idx in faces:
        f = bm.faces.new([verts[i] for i in idx])
        f.smooth = False
        f.normal_update()
        for loop in f.loops:
            loop[col] = (color[0], color[1], color[2], 1.0)
            loop[uv].uv = cube_uv(loop.vert.co, f.normal, tile)


def add_box(bm, col, uv, center, half, color, tile=TILE_WOOD):
    """In-game center+half (x=width, y=up, z=depth) -> Blender (Z-up):
    blender_x = x, blender_y = -z, blender_z = y."""
    cx, cy, cz = center
    hx, hy, hz = half
    lo = (cx - hx, -(cz + hz), cy - hy)
    hi = (cx + hx, -(cz - hz), cy + hy)
    add_box_blender(bm, col, uv, lo, hi, color, tile)


def build_workbench():
    bpy.ops.wm.read_homefile(use_empty=True)
    mesh = bpy.data.meshes.new("workbench_t1")
    obj = bpy.data.objects.new("workbench_t1", mesh)
    bpy.context.collection.objects.link(obj)
    bm = bmesh.new()
    col = bm.loops.layers.float_color.new("Color")
    uv = bm.loops.layers.uv.new("UVMap")

    # Footprint 1.10 x ~0.74, height ~0.90. x = length, z = depth.
    HX = 0.52          # half length of the top
    HZ = 0.34          # half depth of the top
    TOP_Y = 0.78       # top-slab centre height
    TOP_HY = 0.055     # half thickness of the slab
    UNDER = TOP_Y - TOP_HY
    LEG_HX = LEG_HZ = 0.055
    LEG_INSET = 0.07
    lx = HX - LEG_INSET - LEG_HX
    lz = HZ - LEG_INSET - LEG_HZ

    # Plank top: three boards with thin gaps, so seams read even before texture.
    boards = 3
    gap = 0.012
    bw = (2 * HZ - (boards - 1) * gap) / boards
    for i in range(boards):
        cz = -HZ + bw / 2 + i * (bw + gap)
        add_box(bm, col, uv, (0.0, TOP_Y, cz), (HX, TOP_HY, bw / 2), WOOD)
    # A front edge lip (apron face) so the bench reads thick from the front.
    add_box(bm, col, uv, (0.0, UNDER - 0.045, -HZ + 0.03), (HX - 0.02, 0.05, 0.03), WOOD)

    # Four legs.
    for sx in (-1, 1):
        for sz in (-1, 1):
            add_box(bm, col, uv, (sx * lx, UNDER / 2.0, sz * lz),
                    (LEG_HX, UNDER / 2.0, LEG_HZ), WOOD)
    # Aprons / stretchers just under the top (front+back along x, sides along z).
    rail_y = UNDER - 0.10
    add_box(bm, col, uv, (0.0, rail_y, -lz), (lx, 0.05, 0.028), WOOD)
    add_box(bm, col, uv, (0.0, rail_y, lz), (lx, 0.05, 0.028), WOOD)
    add_box(bm, col, uv, (-lx, rail_y, 0.0), (0.028, 0.05, lz), WOOD)
    add_box(bm, col, uv, (lx, rail_y, 0.0), (0.028, 0.05, lz), WOOD)
    # Lower shelf (a plank deck) + its two support rails.
    shelf_y = 0.20
    add_box(bm, col, uv, (0.0, shelf_y, 0.0), (lx, 0.022, lz), WOOD_DK)
    add_box(bm, col, uv, (0.0, shelf_y - 0.03, -lz), (lx, 0.03, 0.026), WOOD)
    add_box(bm, col, uv, (0.0, shelf_y - 0.03, lz), (lx, 0.03, 0.026), WOOD)

    bmesh.ops.recalc_face_normals(bm, faces=list(bm.faces))
    bm.normal_update()
    # Global edge bevel: rounds every box independently (they're never welded),
    # softening the silhouette so the cel ink-edge reads as hand-drawn rather
    # than hard CAD corners. Bevel interpolates COLOR_0 onto the new facets, so
    # colour first; re-project UVs afterwards so the new facets tile cleanly.
    bmesh.ops.bevel(bm, geom=list(bm.edges), offset=0.013, segments=2,
                    affect="EDGES", clamp_overlap=True)
    bm.normal_update()
    for f in bm.faces:
        f.normal_update()
        for loop in f.loops:
            loop[uv].uv = cube_uv(loop.vert.co, f.normal, TILE_WOOD)
    bm.to_mesh(mesh)
    bm.free()
    if "Color" in mesh.color_attributes:
        i = mesh.color_attributes.find("Color")
        mesh.color_attributes.render_color_index = i
        mesh.color_attributes.active_color_index = i
    m = bpy.data.materials.new("workbench_t1_mat")
    m.use_nodes = True
    bsdf = m.node_tree.nodes.get("Principled BSDF")
    vc = m.node_tree.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    m.node_tree.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    mesh.materials.append(m)
    out = os.path.join(ITEMS, "workbench_t1", "model.glb")
    export_glb(obj, out)
    print("built workbench")


def main():
    build_workbench()
    build_sleeping_bag()
    # Furnace is organically displaced already, so it keeps its hard mesh; the
    # boxy props get a light bevel so the cel edge inks every corner.
    unwrap_glb("crude_furnace/model.glb", TILE_STONE)
    unwrap_glb("storage_box_small/model.glb", TILE_WOOD, bevel=0.012)
    unwrap_glb("storage_box_large/model.glb", TILE_WOOD, bevel=0.012)
    unwrap_glb("tool_cupboard/model.glb", TILE_WOOD, bevel=0.012)
    unwrap_glb("torch/model.glb", TILE_WOOD)


if __name__ == "__main__":
    main()
