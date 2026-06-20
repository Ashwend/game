"""Parametric headless-Blender builder for the building-piece glbs: the six
pieces (foundation, wall, window wall, doorway, ceiling, stairs) at each of the
three tiers (sticks / hewn wood / stone), replacing the procedural
`building_piece_mesh`.

Precedent: art/trees/build_tree.py + art/building/build_door.py. Run headless:
  /Applications/Blender.app/Contents/MacOS/Blender --background --python art/building/build_pieces.py

PARITY POINT: the box layout here MIRRORS `crate::building::piece_local_boxes`
(+ the foundation skirt and stair steps). The visual silhouette must match the
collider, so these constants must stay in sync with src/building.rs. Geometry is
built in IN-GAME coordinates (x = width, y = up, z = thickness) and transformed
to Blender (Z-up) on the way into the mesh; export_yup then lands it back in the
in-game frame.

Each glb is a single primitive carrying POSITION/NORMAL/COLOR_0/TEXCOORD_0. The
tier texture (sticks/wood/stone) is applied by Rust as a base-white
StandardMaterial; COLOR_0 multiplies it (mostly white so the texture reads, with
AO on the foundation under-structure). Tiers differ in CONSTRUCTION: sticks is an
open lashed-pole lattice, wood and stone are solid (the texture carries the plank
/ coursing detail).
"""

import bpy
import bmesh
import os

# --- crate::building constants (keep in sync) ---------------------------------
FOUNDATION_SIZE = 3.0
FH = FOUNDATION_SIZE / 2.0           # foundation/cell half-width
FOUNDATION_HEIGHT = 0.5
WALL_HEIGHT = 3.0
TH = 0.1                             # wall half thickness (WALL_THICKNESS_M/2)
CEILING_HH = 0.1                     # CEILING_THICKNESS_M/2
SKIRT = 1.55                         # FOUNDATION_RAISE_MAX_M + 0.05
STAIR_STEPS = 8
STAIR_RISE = WALL_HEIGHT
# Window opening
WIN_HW = 0.5
WIN_JAMB_HW = (FH - WIN_HW) / 2.0
WIN_JAMB_CX = WIN_HW + WIN_JAMB_HW
WIN_SILL_HH = 1.1 / 2.0
WIN_TOP = 1.1 + 1.1
WIN_HEADER_HH = (WALL_HEIGHT - WIN_TOP) / 2.0
# Doorway opening
DOOR_HW = 1.1 / 2.0
DOOR_JAMB_HW = (FH - DOOR_HW) / 2.0
DOOR_JAMB_CX = DOOR_HW + DOOR_JAMB_HW
DOOR_OPEN_H = 2.2
DOOR_HEADER_HH = (WALL_HEIGHT - DOOR_OPEN_H) / 2.0

TILE_M = 1.5  # texture tile size (metres per repeat)

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

WHITE = (1.0, 1.0, 1.0)

PIECES = ["foundation", "wall", "window_wall", "doorway", "ceiling", "stairs"]
TIERS = ["sticks", "wood", "stone"]


def wall_like_segments(piece):
    """In-game (center, half) boxes for the wall-like pieces, mirroring
    `piece_local_boxes`."""
    if piece == "wall":
        return [((0.0, WALL_HEIGHT / 2.0, 0.0), (FH, WALL_HEIGHT / 2.0, TH))]
    if piece == "window_wall":
        return [
            ((-WIN_JAMB_CX, WALL_HEIGHT / 2.0, 0.0), (WIN_JAMB_HW, WALL_HEIGHT / 2.0, TH)),
            ((WIN_JAMB_CX, WALL_HEIGHT / 2.0, 0.0), (WIN_JAMB_HW, WALL_HEIGHT / 2.0, TH)),
            ((0.0, WIN_SILL_HH, 0.0), (WIN_HW, WIN_SILL_HH, TH)),
            ((0.0, WIN_TOP + WIN_HEADER_HH, 0.0), (WIN_HW, WIN_HEADER_HH, TH)),
        ]
    if piece == "doorway":
        return [
            ((-DOOR_JAMB_CX, WALL_HEIGHT / 2.0, 0.0), (DOOR_JAMB_HW, WALL_HEIGHT / 2.0, TH)),
            ((DOOR_JAMB_CX, WALL_HEIGHT / 2.0, 0.0), (DOOR_JAMB_HW, WALL_HEIGHT / 2.0, TH)),
            ((0.0, DOOR_OPEN_H + DOOR_HEADER_HH, 0.0), (DOOR_HW, DOOR_HEADER_HH, TH)),
        ]
    return []


def stair_segments():
    rise = STAIR_RISE / STAIR_STEPS
    depth = FOUNDATION_SIZE / STAIR_STEPS
    out = []
    for i in range(STAIR_STEPS):
        tread_top = rise * (i + 1)
        cz = -FOUNDATION_SIZE / 2.0 + depth * (i + 0.5)
        out.append(((0.0, tread_top / 2.0, cz), (FH, tread_top / 2.0, depth / 2.0)))
    return out


# ------------------------------------------------------------------ mesh helpers
def cube_uv(co, normal, tile):
    ax, ay, az = abs(normal[0]), abs(normal[1]), abs(normal[2])
    if ax >= ay and ax >= az:
        u, v = co[1], co[2]
    elif ay >= ax and ay >= az:
        u, v = co[0], co[2]
    else:
        u, v = co[0], co[1]
    return (u / tile, v / tile)


def add_box_blender(bm, col, uv, lo, hi, color):
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
            loop[uv].uv = cube_uv(loop.vert.co, f.normal, TILE_M)


def add_box(bm, col, uv, center, half, color):
    """Add a box given an IN-GAME center+half; transform to Blender
    (Blender_X = x, Blender_Y = -z, Blender_Z = y)."""
    cx, cy, cz = center
    hx, hy, hz = half
    lo = (cx - hx, -(cz + hz), cy - hy)
    hi = (cx + hx, -(cz - hz), cy + hy)
    add_box_blender(bm, col, uv, lo, hi, color)


# ------------------------------------------------------------------ tier renders
def solid_segment(bm, col, uv, center, half, color=WHITE):
    add_box(bm, col, uv, center, half, color)


def lattice_segment(bm, col, uv, center, half):
    """Open lashed-pole lattice filling a wall segment: top/bottom rails plus
    sparse vertical poles with real gaps. Collider stays the solid box; the gaps
    are purely visual (the sticks-tier 'barely holding together' read)."""
    cx, cy, cz = center
    hx, hy, hz = half
    rail_h = 0.05
    add_box(bm, col, uv, (cx, cy + hy - rail_h, cz), (hx, rail_h, hz + 0.012), WHITE)
    add_box(bm, col, uv, (cx, cy - hy + rail_h, cz), (hx, rail_h, hz + 0.012), WHITE)
    if hy > 0.9:
        add_box(bm, col, uv, (cx, cy, cz), (hx, rail_h, hz + 0.012), WHITE)
    spacing = 0.30
    count = max(1, int((2.0 * hx - 0.12) / spacing))
    start_x = cx - (count - 1) * spacing / 2.0
    for i in range(count):
        x = start_x + i * spacing
        add_box(bm, col, uv, (x, cy, cz), (0.034, hy - 0.015, 0.034), WHITE)


def build_foundation(bm, col, uv, tier):
    if tier == "sticks":
        # Two beams along Z, a deck of logs along X, four corner stilts to ground.
        for x in (-FH + 0.5, FH - 0.5):
            add_box(bm, col, uv, (x, 0.14, 0.0), (0.12, 0.14, FH - 0.05), WHITE)
        spacing = 0.26
        count = int((2.0 * FH) / spacing)
        start_z = -(count - 1) * spacing / 2.0
        for i in range(count):
            z = start_z + i * spacing
            add_box(bm, col, uv, (0.0, FOUNDATION_HEIGHT - 0.10, z), (FH - 0.02, 0.095, 0.105), WHITE)
        for (x, z) in [(-FH + 0.12, -FH + 0.12), (FH - 0.12, -FH + 0.12),
                       (-FH + 0.12, FH - 0.12), (FH - 0.12, FH - 0.12)]:
            add_box(bm, col, uv, (x, (FOUNDATION_HEIGHT - SKIRT) / 2.0, z),
                    (0.06, (FOUNDATION_HEIGHT + SKIRT) / 2.0, 0.06), (0.7, 0.7, 0.7))
    else:
        # Solid slab + a recessed plinth down to the ground (darker = shadowed
        # under-structure for raised platforms).
        add_box(bm, col, uv, (0.0, FOUNDATION_HEIGHT / 2.0, 0.0), (FH, FOUNDATION_HEIGHT / 2.0, FH), WHITE)
        add_box(bm, col, uv, (0.0, (0.02 - SKIRT) / 2.0, 0.0),
                (FH - 0.04, (SKIRT + 0.02) / 2.0, FH - 0.04), (0.5, 0.5, 0.5))


def build_ceiling(bm, col, uv, tier):
    if tier == "sticks":
        # Open joists: cross logs spanning the cell.
        spacing = 0.34
        count = int((2.0 * FH) / spacing)
        start_z = -(count - 1) * spacing / 2.0
        for i in range(count):
            z = start_z + i * spacing
            add_box(bm, col, uv, (0.0, CEILING_HH, z), (FH - 0.02, 0.06, 0.075), WHITE)
    else:
        add_box(bm, col, uv, (0.0, CEILING_HH, 0.0), (FH, CEILING_HH, FH), WHITE)


def build_piece(piece, tier):
    bpy.ops.wm.read_homefile(use_empty=True)
    mesh = bpy.data.meshes.new(f"{piece}_{tier}")
    obj = bpy.data.objects.new(f"{piece}_{tier}", mesh)
    bpy.context.collection.objects.link(obj)
    bm = bmesh.new()
    col = bm.loops.layers.float_color.new("Color")
    uv = bm.loops.layers.uv.new("UVMap")

    if piece == "foundation":
        build_foundation(bm, col, uv, tier)
    elif piece == "ceiling":
        build_ceiling(bm, col, uv, tier)
    elif piece == "stairs":
        for c, h in stair_segments():
            solid_segment(bm, col, uv, c, h)
    else:
        for c, h in wall_like_segments(piece):
            if tier == "sticks":
                lattice_segment(bm, col, uv, c, h)
            else:
                solid_segment(bm, col, uv, c, h)

    # Flip every face outward: add_box winds boxes inward, and Bevy
    # backface-culls, so without this the pieces render inside-out in game
    # (you see the far interior faces). Each box is its own closed component,
    # so recalc orients them all correctly.
    bmesh.ops.recalc_face_normals(bm, faces=list(bm.faces))
    bm.normal_update()
    bm.to_mesh(mesh)
    bm.free()
    if "Color" in mesh.color_attributes:
        i = mesh.color_attributes.find("Color")
        mesh.color_attributes.render_color_index = i
        mesh.color_attributes.active_color_index = i
    m = bpy.data.materials.new(f"{piece}_{tier}_mat")
    m.use_nodes = True
    bsdf = m.node_tree.nodes.get("Principled BSDF")
    vc = m.node_tree.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    m.node_tree.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    mesh.materials.append(m)
    return obj


def export(obj, out_path):
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=out_path, export_format="GLB", use_selection=True,
        export_yup=True, export_apply=True, export_normals=True,
        export_materials="EXPORT", export_texcoords=True,
    )


def main():
    for piece in PIECES:
        for tier in TIERS:
            obj = build_piece(piece, tier)
            out = os.path.join(REPO, "assets", "building", f"{piece}_{tier}.glb")
            export(obj, out)
            print("exported", out)


if __name__ == "__main__":
    main()
