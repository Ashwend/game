"""Parametric headless-Blender builder for the door panel glbs (wood + iron),
a matched family that replaces the old procedural `door_panel_mesh`.

Precedent: art/trees/build_tree.py. Run headless:
  /Applications/Blender.app/Contents/MacOS/Blender --background --python art/building/build_door.py

Reference frame (must match crate::building DOOR_PANEL_* + the collider +
`animate_door_panels_system`): the panel's HINGE edge sits at the local origin
and the panel spans local +X to DOOR_PANEL_WIDTH_M; height runs 0..HEIGHT (base
on the floor); thickness is +/-THICKNESS/2. The child panel entity pivots about
this origin, so the hinge MUST be at x=0.

Blender is Z-up and we export with export_yup=True, so:
  in-game +X (width, hinge->swing edge) = Blender +X
  in-game +Y (up,   height)             = Blender +Z
  in-game +Z (thickness)                = Blender -Y
We therefore build width along Blender +X, height along Blender +Z, thickness
along Blender +/-Y.

Each glb is a SINGLE primitive carrying POSITION/NORMAL/COLOR_0/TEXCOORD_0. The
texture is NOT baked into the glb material; Rust builds a textured
StandardMaterial (door_wood.png / door_iron.png) and the COLOR_0 vertex colours
multiply it per part (frame/braces/straps tinted, plank/plate field neutral),
exactly the tree-canopy pattern.
"""

import bpy
import bmesh
import math
import os

# --- crate::building constants (keep in sync) ---------------------------------
W = 1.04   # DOOR_PANEL_WIDTH_M
H = 2.14   # DOOR_PANEL_HEIGHT_M
T = 0.08   # DOOR_PANEL_THICKNESS_M
HT = T / 2.0

# Texture tile size in metres (one full texture repeat). Tuned so the wood
# plank texture (~19 planks/tile) reads as ~6-7 boards across the door.
TILE_M = 3.0

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def reset_scene():
    bpy.ops.wm.read_homefile(use_empty=True)


def cube_uv(co, normal, tile):
    """Cube/triplanar projection: pick the plane perpendicular to the face's
    dominant axis so the texture tiles seamlessly across adjacent boxes."""
    ax, ay, az = abs(normal[0]), abs(normal[1]), abs(normal[2])
    if ax >= ay and ax >= az:        # +/-X face -> project (Y, Z)
        u, v = co[1], co[2]
    elif ay >= ax and ay >= az:      # +/-Y face (front/back) -> project (X, Z)
        u, v = co[0], co[2]
    else:                            # +/-Z face -> project (X, Y)
        u, v = co[0], co[1]
    return (u / tile, v / tile)


def add_box(bm, col_layer, uv_layer, lo, hi, color, tile=TILE_M):
    """Add an axis-aligned box [lo..hi] with a flat-shaded COLOR_0 and
    cube-projected UVs."""
    x0, y0, z0 = lo
    x1, y1, z1 = hi
    verts = [
        bm.verts.new((x0, y0, z0)), bm.verts.new((x1, y0, z0)),
        bm.verts.new((x1, y1, z0)), bm.verts.new((x0, y1, z0)),
        bm.verts.new((x0, y0, z1)), bm.verts.new((x1, y0, z1)),
        bm.verts.new((x1, y1, z1)), bm.verts.new((x0, y1, z1)),
    ]
    faces = [
        (0, 1, 2, 3), (4, 7, 6, 5), (0, 4, 5, 1),
        (2, 6, 7, 3), (1, 5, 6, 2), (0, 3, 7, 4),
    ]
    for idx in faces:
        f = bm.faces.new([verts[i] for i in idx])
        f.smooth = False
        f.normal_update()
        for loop in f.loops:
            loop[col_layer] = (color[0], color[1], color[2], 1.0)
            loop[uv_layer].uv = cube_uv(loop.vert.co, f.normal, tile)


def frame(bm, col, uv, c_frame):
    """The shared door frame: four proud border bars around the perimeter,
    slightly thicker than the panel so it reads as a banded edge. Identical
    for both variants so the family matches."""
    fw = 0.085          # frame bar width
    pr = HT + 0.012     # proud thickness (sticks out past the panel face)
    # Left + right stiles (full height).
    add_box(bm, col, uv, (0.0, -pr, 0.0), (fw, pr, H), c_frame)
    add_box(bm, col, uv, (W - fw, -pr, 0.0), (W, pr, H), c_frame)
    # Top + bottom rails (between the stiles).
    add_box(bm, col, uv, (fw, -pr, H - fw), (W - fw, pr, H), c_frame)
    add_box(bm, col, uv, (fw, -pr, 0.0), (W - fw, pr, fw), c_frame)


def rivet_row(bm, col, uv, z, color):
    """A row of small proud bolt-heads across a strap at height z."""
    pr = HT + 0.026
    rw = 0.035
    for x in (0.16, W * 0.5, W - 0.16):
        add_box(bm, col, uv, (x - rw, -pr, z - rw), (x + rw, pr, z + rw), color)


def build_wood(bm, col, uv):
    # Plank field: one flat slab; the door_wood texture supplies the boards.
    add_box(bm, col, uv, (0.0, -HT, 0.0), (W, HT, H), (1.0, 1.0, 1.0))
    # Frame (warm, slightly darker than the boards so the border reads).
    frame(bm, col, uv, (0.82, 0.74, 0.62))
    # Ledge-and-brace back (the icon's Z-brace): two horizontal ledges + a
    # diagonal, proud and darker.
    c_brace = (0.55, 0.45, 0.34)
    pr = HT + 0.018
    bw = 0.10
    add_box(bm, col, uv, (0.10, -pr, 0.40), (W - 0.10, pr, 0.40 + bw), c_brace)
    add_box(bm, col, uv, (0.10, -pr, H - 0.62), (W - 0.10, pr, H - 0.62 + bw), c_brace)
    # Diagonal brace: a thin slab stepped across the gap between the ledges.
    steps = 9
    z0, z1 = 0.40 + bw, H - 0.62
    for i in range(steps):
        t0 = i / steps
        cx = 0.12 + t0 * (W - 0.24)
        cz = z0 + t0 * (z1 - z0)
        add_box(bm, col, uv, (cx - 0.06, -pr, cz - 0.06), (cx + 0.06, pr, cz + 0.07), c_brace)
    # Iron ring-pull handle on the swing edge, mid-height (cool grey nub).
    hx = W - 0.16
    add_box(bm, col, uv, (hx - 0.035, -HT - 0.05, 1.02), (hx + 0.035, HT + 0.05, 1.14),
            (0.42, 0.44, 0.47))


def build_iron(bm, col, uv):
    # Plate field: one flat slab; door_iron texture supplies the steel.
    add_box(bm, col, uv, (0.0, -HT, 0.0), (W, HT, H), (1.0, 1.0, 1.0))
    # Frame (brighter, cooler than the plate).
    frame(bm, col, uv, (0.92, 0.95, 1.0))
    # Three horizontal riveted straps across the door.
    c_strap = (0.70, 0.73, 0.78)
    c_rivet = (0.95, 0.97, 1.0)
    pr = HT + 0.02
    sw = 0.12
    for z in (H * 0.22, H * 0.5, H * 0.78):
        add_box(bm, col, uv, (0.07, -pr, z - sw / 2), (W - 0.07, pr, z + sw / 2), c_strap)
        rivet_row(bm, col, uv, z, c_rivet)
    # Heavy ring-pull on the swing edge.
    hx = W - 0.17
    add_box(bm, col, uv, (hx - 0.045, -HT - 0.055, 1.0), (hx + 0.045, HT + 0.055, 1.16),
            (0.88, 0.90, 0.95))


def build_door(kind):
    reset_scene()
    mesh = bpy.data.meshes.new(f"{kind}_door")
    obj = bpy.data.objects.new(f"{kind}_door", mesh)
    bpy.context.collection.objects.link(obj)

    bm = bmesh.new()
    col = bm.loops.layers.float_color.new("Color")
    uv = bm.loops.layers.uv.new("UVMap")

    if kind == "wood":
        build_wood(bm, col, uv)
    else:
        build_iron(bm, col, uv)

    # Flip every face outward: add_box winds boxes inward, and Bevy
    # backface-culls, so without this the panel renders inside-out in game.
    # Each box is its own closed component, so recalc orients them correctly.
    bmesh.ops.recalc_face_normals(bm, faces=list(bm.faces))
    bm.normal_update()
    bm.to_mesh(mesh)
    bm.free()

    # Make "Color" the active render attribute so COLOR_0 exports non-white.
    if "Color" in mesh.color_attributes:
        mesh.color_attributes.render_color_index = mesh.color_attributes.find("Color")
        mesh.color_attributes.active_color_index = mesh.color_attributes.find("Color")

    # A throwaway material with a Vertex Color node so COLOR_0 round-trips on
    # export (Rust replaces this material entirely with a textured one).
    m = bpy.data.materials.new(f"{kind}_door_mat")
    m.use_nodes = True
    nt = m.node_tree
    bsdf = nt.nodes.get("Principled BSDF")
    vc = nt.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    mesh.materials.append(m)

    return obj


def export(obj, out_path):
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=out_path,
        export_format="GLB",
        use_selection=True,
        export_yup=True,
        export_apply=True,
        export_normals=True,
        export_materials="EXPORT",
        export_texcoords=True,
    )
    print(f"exported {out_path}")


def main():
    for kind, item in (("wood", "hewn_log_door"), ("iron", "iron_door")):
        obj = build_door(kind)
        out = os.path.join(REPO, "assets", "items", item, "model.glb")
        export(obj, out)


if __name__ == "__main__":
    main()
