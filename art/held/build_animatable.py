#!/usr/bin/env python3
"""Blender headless: TRELLIS lowpoly -> ANIMATABLE multi-primitive held glbs.

Run (from art/held/):
  Blender --background --python build_animatable.py -- <wooden_bow|crossbow|bandage>

The bow, crossbow, and bandage viewmodels animate per-piece (limb flex, string
slide, tail unroll), so unlike the static batch-2 rebuilds they cannot ship as
one TRELLIS primitive. This script rebuilds each as the SAME primitive layout
the engine's rig math expects (src/app/systems/items/held/ranged_viewmodel.rs
constants, all expressed in the OLD authored frames):

  wooden_bow  6 prims: grip / limb_upper / limb_lower / string_upper /
              string_lower / arrow. The TRELLIS stave is fitted so its tips
              land EXACTLY on the authored anchors (0.16, +/-0.45, 0) glTF
              (two-point similarity fit), its reconstructed string is deleted,
              and the stave is bisected at the limb pivots (z = +/-0.085
              authoring) into grip + limbs. The string legs are rebuilt
              procedurally on the authored geometry (they are pure rig pieces:
              slim boxes from tip to nock), and the nocked-arrow primitive is
              REUSED from the old authored glb (its nock end is welded to the
              authored rest nock by the rig).
  crossbow    3 prims: body (TRELLIS stock+prod fused) / string / bolt. The
              body is fitted to the old tiller length with the prod up, the
              reconstructed string deleted; the string is rebuilt as two legs
              from the fitted prod tips to the authored cocked nut (authoring
              z 0.115), and the bolt is reused from the old glb.
  bandage     2 prims: roll / tail, split from the one TRELLIS mesh. The roll
              is fitted so its bottom tangent sits at the authored tail pivot
              (radius 0.10), the tail (the strip beyond the roll surface)
              becomes the BandageTail prim, authored at whatever extension the
              reconstruction grew (the rig scales it from an 0.18 rest stub to
              full).

TRELLIS prims ship white COLOR_0 + the extracted albedo (the `Baked` family);
the procedural strings ship pale-tan COLOR_0 on the Cord family; reused old
prims keep their authored COLOR_0 on the Wood family. Multi-primitive export
REQUIRES export_materials='EXPORT' (NONE collapses the split), so every slot
gets a lean placeholder material and the TRELLIS texture is extracted to a
separate albedo instead of riding embedded in the glb.
"""
import json
import math
import os
import sys

import bpy
import bmesh
from mathutils import Matrix, Vector

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
assert argv, "pass an item: wooden_bow | crossbow | bandage"
ITEM = argv[0]

OLD_GLB = f"/Users/dannie/Desktop/dev/game/assets/items/{ITEM}/model.glb"

# Manual orientation knobs (degrees, applied before the auto steps) for when a
# reconstruction arrives in an unexpected pose; iterate against the turntable.
KNOBS = {
    "wooden_bow": dict(rot_x_deg=0, rot_y_deg=0, rot_z_deg=0),
    "crossbow": dict(rot_x_deg=0, rot_y_deg=0, rot_z_deg=0),
    "bandage": dict(rot_x_deg=0, rot_y_deg=0, rot_z_deg=0),
}


def log(msg):
    print(f"[build_animatable] {msg}", flush=True)


# --------------------------------------------------------------------------- #
# Shared helpers
# --------------------------------------------------------------------------- #

def import_lowpoly():
    bpy.ops.import_scene.gltf(filepath=f"meshes/{ITEM}_lowpoly.glb")
    objs = [o for o in bpy.context.selected_objects if o.type == "MESH"]
    assert objs, "no mesh in lowpoly glb"
    obj = objs[0]
    obj.parent = None
    obj.matrix_world = Matrix.Identity(4)
    return obj


def import_old_prim(material_name):
    """One primitive of the OLD authored glb, as its own object, geometry
    kept in the authored frame (identity transform)."""
    before = set(bpy.data.objects)
    bpy.ops.import_scene.gltf(filepath=OLD_GLB)
    new = [o for o in set(bpy.data.objects) - before if o.type == "MESH"]
    assert new, "old glb import produced no mesh"
    src = new[0]
    src.matrix_world = Matrix.Identity(4)
    slot = next(i for i, m in enumerate(src.data.materials)
                if m and material_name in m.name)
    bpy.context.view_layer.objects.active = src
    for o in bpy.data.objects:
        o.select_set(o is src)
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.select_all(action="DESELECT")
    bpy.ops.object.mode_set(mode="OBJECT")
    for poly in src.data.polygons:
        poly.select = poly.material_index == slot
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.separate(type="SELECTED")
    bpy.ops.object.mode_set(mode="OBJECT")
    part = [o for o in bpy.context.selected_objects if o is not src][0]
    # Drop the other imported leftovers.
    for o in new:
        if o is not part and o.name in bpy.data.objects:
            bpy.data.objects.remove(o, do_unlink=True)
    part.matrix_world = Matrix.Identity(4)
    normalize_color(part)
    return part


def pca_align(me):
    """Major axis -> Z, mid -> X, minor -> Y (same convention as build_held)."""
    import numpy as np
    pts = np.array([(v.co.x, v.co.y, v.co.z) for v in me.vertices])
    pts -= pts.mean(axis=0)
    cov = pts.T @ pts / len(pts)
    _, evecs = np.linalg.eigh(cov)
    v_minor, v_mid, v_major = evecs[:, 0], evecs[:, 1], evecs[:, 2]
    rows = np.stack([v_mid, v_minor, v_major])
    if np.linalg.det(rows) < 0:
        rows[1] = -rows[1]
    me.transform(Matrix(np.vstack([np.hstack([rows, np.zeros((3, 1))]),
                                   [0, 0, 0, 1]]).tolist()))
    me.update()


def apply_knobs(me):
    k = KNOBS[ITEM]
    for axis, key in (("X", "rot_x_deg"), ("Y", "rot_y_deg"), ("Z", "rot_z_deg")):
        if k[key]:
            me.transform(Matrix.Rotation(math.radians(k[key]), 4, axis))


def bounds(me):
    xs = [v.co.x for v in me.vertices]
    ys = [v.co.y for v in me.vertices]
    zs = [v.co.z for v in me.vertices]
    return (min(xs), max(xs)), (min(ys), max(ys)), (min(zs), max(zs))


def delete_near_segment(me, p0, p1, radius, z_guard):
    """Delete faces whose every vertex lies within `radius` of the segment
    p0-p1 AND inside the |z| < z_guard band (the reconstructed string chord,
    keeping the junctions at the tips), then CAP the openings the cut leaves
    (boundary loops that were welded to the deleted string), so the stave
    never shows see-through holes where the string attached."""
    seg = p1 - p0
    seg_len2 = seg.length_squared

    def near(co):
        t = max(0.0, min(1.0, (co - p0).dot(seg) / seg_len2))
        return (co - (p0 + seg * t)).length < radius and abs(co.z) < z_guard

    bm = bmesh.new()
    bm.from_mesh(me)
    doomed = [f for f in bm.faces if all(near(v.co) for v in f.verts)]
    if doomed:
        bmesh.ops.delete(bm, geom=doomed, context="FACES")
        loose = [v for v in bm.verts if not v.link_faces]
        if loose:
            bmesh.ops.delete(bm, geom=loose, context="VERTS")
        boundary = [e for e in bm.edges if e.is_boundary]
        if boundary:
            bmesh.ops.holes_fill(bm, edges=boundary)
        bm.to_mesh(me)
        me.update()
    bm.free()
    log(f"  deleted {len(doomed)} string faces, capped the cut")


def bisect_part(obj, plane_co, plane_no, keep_positive):
    """A duplicate of obj cut at the plane, keeping one side."""
    dup = obj.copy()
    dup.data = obj.data.copy()
    bpy.context.scene.collection.objects.link(dup)
    bm = bmesh.new()
    bm.from_mesh(dup.data)
    geom = bm.verts[:] + bm.edges[:] + bm.faces[:]
    bmesh.ops.bisect_plane(
        bm, geom=geom, plane_co=plane_co, plane_no=plane_no,
        clear_outer=not keep_positive, clear_inner=keep_positive,
    )
    bm.to_mesh(dup.data)
    dup.data.update()
    bm.free()
    return dup


def set_color(obj, rgba):
    me = obj.data
    for attr in list(me.color_attributes):
        me.color_attributes.remove(attr)
    attr = me.color_attributes.new("Color", "FLOAT_COLOR", "CORNER")
    vals = []
    for _ in range(len(attr.data)):
        vals.extend(rgba)
    attr.data.foreach_set("color", vals)


def normalize_color(obj):
    """Resample whatever color attribute the old glb imported with into the
    canonical FLOAT_COLOR/CORNER "Color" attribute. Joining meshes whose color
    attributes differ in name/domain/type silently drops the mismatched ones
    (the reused arrow rendered WHITE), so every part is normalized first."""
    me = obj.data
    src = me.color_attributes.active_color or (
        me.color_attributes[0] if me.color_attributes else None)
    if src is None:
        set_color(obj, (1.0, 1.0, 1.0, 1.0))
        return
    if src.name == "Color" and src.data_type == "FLOAT_COLOR" and src.domain == "CORNER":
        return
    if src.domain == "CORNER":
        vals = [c for d in src.data for c in d.color]
    else:  # POINT: expand to corners via loops
        per_vert = [tuple(d.color) for d in src.data]
        vals = [c for loop in me.loops for c in per_vert[loop.vertex_index]]
    for attr in list(me.color_attributes):
        me.color_attributes.remove(attr)
    attr = me.color_attributes.new("Color", "FLOAT_COLOR", "CORNER")
    attr.data.foreach_set("color", vals)


def ensure_uvs(obj):
    if not obj.data.uv_layers:
        obj.data.uv_layers.new(name="UVMap")


def fresh_material(name):
    mat = bpy.data.materials.new(name)
    mat.use_nodes = False
    return mat


def make_leg(name, p0, p1, half_a, axis_a, half_b, axis_b):
    """A slim box running p0 -> p1 with rectangular cross-section spanned by
    half_a*axis_a and half_b*axis_b."""
    verts = []
    for end in (p0, p1):
        for sa in (-1, 1):
            for sb in (-1, 1):
                verts.append(end + axis_a * (half_a * sa) + axis_b * (half_b * sb))
    faces = [
        (0, 1, 3, 2), (4, 6, 7, 5),          # end caps
        (0, 2, 6, 4), (1, 5, 7, 3),          # sides
        (0, 4, 5, 1), (2, 3, 7, 6),
    ]
    me = bpy.data.meshes.new(name)
    me.from_pydata([tuple(v) for v in verts], [], faces)
    me.update()
    obj = bpy.data.objects.new(name, me)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def extract_albedo(obj):
    for mat in obj.data.materials:
        if not mat or not mat.use_nodes:
            continue
        for node in mat.node_tree.nodes:
            if node.type != "BSDF_PRINCIPLED":
                continue
            for link in node.inputs["Base Color"].links:
                if link.from_node.type == "TEX_IMAGE" and link.from_node.image:
                    return link.from_node.image
    images = [i for i in bpy.data.images if i.size[0] > 0]
    assert images, "no albedo image found"
    return max(images, key=lambda i: i.size[0] * i.size[1])


def assemble_and_export(parts):
    """Join `parts` (ordered (obj, material_name) pairs) into one mesh whose
    material-slot order defines the exported primitive order."""
    for obj, mat_name in parts:
        ensure_uvs(obj)
        obj.data.materials.clear()
        obj.data.materials.append(fresh_material(mat_name))
    target = parts[0][0]
    for o in bpy.data.objects:
        o.select_set(False)
    for obj, _ in parts:
        obj.select_set(True)
    bpy.context.view_layer.objects.active = target
    if len(parts) > 1:
        bpy.ops.object.join()
    target.name = ITEM

    os.makedirs(f"meshes/game/{ITEM}", exist_ok=True)
    for o in bpy.data.objects:
        o.select_set(o is target)
    kwargs = dict(filepath=f"meshes/game/{ITEM}/model.glb", use_selection=True,
                  export_materials="EXPORT")
    try:
        bpy.ops.export_scene.gltf(**kwargs, export_vertex_color="ACTIVE")
    except TypeError:
        bpy.ops.export_scene.gltf(**kwargs)
    zs = [round(v, 3) for v in bounds(target.data)[2]]
    log(f"exported meshes/game/{ITEM}/model.glb (slots="
        f"{[m.name for m in target.data.materials]}, z {zs})")


# --------------------------------------------------------------------------- #
# Items. All geometry below is in BLENDER coords (importer maps glTF
# (x, y, z) -> Blender (x, -z, y); our authored glTF frames therefore read as
# Blender x = glTF x, Blender y = -glTF z, Blender z = glTF y).
# --------------------------------------------------------------------------- #

def build_bow(raw):
    me = raw.data
    apply_knobs(me)
    pca_align(me)  # stave axis -> Z

    # String side must be +X: the stave belly bulges away from the tip chord,
    # so if the mid-band centroid sits ABOVE the tip mean in x, mirror.
    (x0, x1), _, (z0, z1) = bounds(me)
    tips_x = []
    for band in ((z1 - 0.05 * (z1 - z0), z1), (z0, z0 + 0.05 * (z1 - z0))):
        xs = [v.co.x for v in me.vertices if band[0] <= v.co.z <= band[1]]
        tips_x.append(sum(xs) / len(xs))
    mid_xs = [v.co.x for v in me.vertices if abs(v.co.z - (z0 + z1) / 2) < 0.1 * (z1 - z0)]
    if sum(mid_xs) / len(mid_xs) > sum(tips_x) / 2:
        me.transform(Matrix.Rotation(math.pi, 4, "Z"))
        me.update()

    # Tip points: extreme-z vertices.
    top = max(me.vertices, key=lambda v: v.co.z).co.copy()
    bot = min(me.vertices, key=lambda v: v.co.z).co.copy()

    # Delete the reconstructed string: faces hugging the tip-to-tip chord.
    # The radius stays TIGHT (a string is ~1% of the span; the earlier 3.5%
    # sweep also ate stave faces where the limbs curve toward the chord and
    # left see-through holes, owner report) and the guard keeps a wider
    # keep-zone at the tips where stave and string genuinely meet.
    span = (top - bot).length
    delete_near_segment(me, bot, top, radius=0.028 * span, z_guard=abs(top.z) - 0.12 * span)

    # Two-point similarity fit in the bow plane (X-Z): tips -> authored
    # anchors glTF (0.16, +/-0.45, 0) = Blender (0.16, 0, +/-0.45).
    top = max(me.vertices, key=lambda v: v.co.z).co.copy()
    bot = min(me.vertices, key=lambda v: v.co.z).co.copy()
    t_top, t_bot = Vector((0.16, 0.0, 0.45)), Vector((0.16, 0.0, -0.45))
    src_v, dst_v = top - bot, t_top - t_bot
    scale = dst_v.length / src_v.length
    ang = math.atan2(dst_v.x, dst_v.z) - math.atan2(src_v.x, src_v.z)
    rot = Matrix.Rotation(-ang, 4, "Y")
    me.transform(rot)
    me.transform(Matrix.Scale(scale, 4))
    new_bot = min(me.vertices, key=lambda v: v.co.z).co.copy()
    me.transform(Matrix.Translation(t_bot - new_bot))
    # Flatten depth around 0.
    _, (y0, y1), _ = bounds(me)
    me.transform(Matrix.Translation(Vector((0, -(y0 + y1) / 2, 0))))
    me.update()

    # Split at the limb pivots (authoring z +/-0.085).
    grip = bisect_part(raw, Vector((0, 0, 0.085)), Vector((0, 0, 1)), False)
    grip = bisect_part(grip, Vector((0, 0, -0.085)), Vector((0, 0, 1)), True)
    upper = bisect_part(raw, Vector((0, 0, 0.085)), Vector((0, 0, 1)), True)
    lower = bisect_part(raw, Vector((0, 0, -0.085)), Vector((0, 0, 1)), False)

    albedo = extract_albedo(raw)
    bpy.data.objects.remove(raw, do_unlink=True)
    for part in (grip, upper, lower):
        set_color(part, (1.0, 1.0, 1.0, 1.0))

    # Procedural string legs on the authored geometry: glTF x 0.154..0.166,
    # z +/-0.006 = Blender y -/+0.006, from each tip anchor to the nock.
    cord = (0.85, 0.80, 0.66, 1.0)
    nock = Vector((0.16, 0.0, 0.0))
    su = make_leg("string_upper", Vector((0.16, 0, 0.45)), nock,
                  0.006, Vector((1, 0, 0)), 0.006, Vector((0, 1, 0)))
    sl = make_leg("string_lower", Vector((0.16, 0, -0.45)), nock,
                  0.006, Vector((1, 0, 0)), 0.006, Vector((0, 1, 0)))
    for leg in (su, sl):
        set_color(leg, cord)

    arrow = import_old_prim("arrow")

    assemble_and_export([
        (grip, f"{ITEM}_grip"),
        (upper, f"{ITEM}_limb_upper"),
        (lower, f"{ITEM}_limb_lower"),
        (su, f"{ITEM}_string_upper"),
        (sl, f"{ITEM}_string_lower"),
        (arrow, f"{ITEM}_arrow"),
    ])
    return albedo


def build_crossbow(raw):
    me = raw.data
    apply_knobs(me)
    pca_align(me)  # tiller -> Z; prod spread -> X

    # Prod (the wide end) up: default wider-end-up.
    (x0, x1), _, (z0, z1) = bounds(me)

    def band_width(lo, hi):
        xs = [v.co.x for v in me.vertices if lo <= v.co.z <= hi]
        return (max(xs) - min(xs)) if xs else 0.0

    h = z1 - z0
    if band_width(z0, z0 + 0.3 * h) > band_width(z1 - 0.3 * h, z1):
        me.transform(Matrix.Rotation(math.pi, 4, "X"))
        me.update()
        (x0, x1), _, (z0, z1) = bounds(me)

    # Delete the reconstructed string: the chord between prod tips.
    left = min((v.co for v in me.vertices if v.co.z > z1 - 0.3 * (z1 - z0)),
               key=lambda c: c.x).copy()
    right = max((v.co for v in me.vertices if v.co.z > z1 - 0.3 * (z1 - z0)),
                key=lambda c: c.x).copy()
    delete_near_segment(me, left, right, radius=0.03 * (x1 - x0),
                        z_guard=1e9)

    # Fit: tiller length (z span) -> 0.72, z range -0.42..0.30, centered x/y.
    (x0, x1), (y0, y1), (z0, z1) = bounds(me)
    s = 0.72 / (z1 - z0)
    me.transform(Matrix.Scale(s, 4))
    (x0, x1), (y0, y1), (z0, z1) = bounds(me)
    me.transform(Matrix.Translation(Vector((
        -(x0 + x1) / 2, -(y0 + y1) / 2, -0.42 - z0))))
    me.update()

    albedo = extract_albedo(raw)
    set_color(raw, (1.0, 1.0, 1.0, 1.0))

    # String legs: fitted prod tips -> the authored cocked nut (authoring
    # z 0.115 -> Blender z 0.115), groove side glTF -z = Blender +y.
    (x0, x1), _, (z0, z1) = bounds(raw.data)
    tip_band = [v.co for v in raw.data.vertices if v.co.z > z1 - 0.35 * (z1 - z0)]
    lt = min(tip_band, key=lambda c: c.x).copy()
    rt = max(tip_band, key=lambda c: c.x).copy()
    nut = Vector((0.0, 0.033, 0.115))
    cord = (0.85, 0.80, 0.66, 1.0)
    sl = make_leg("string_left", lt, nut, 0.008, Vector((0, 0, 1)),
                  0.008, Vector((0, 1, 0)))
    sr = make_leg("string_right", rt, nut, 0.008, Vector((0, 0, 1)),
                  0.008, Vector((0, 1, 0)))
    for leg in (sl, sr):
        set_color(leg, cord)
    # Both legs are ONE primitive (the rig translates the string as a whole).
    for o in bpy.data.objects:
        o.select_set(o in (sl, sr))
    bpy.context.view_layer.objects.active = sl
    bpy.ops.object.join()
    string = sl

    bolt = import_old_prim("bolt")

    assemble_and_export([
        (raw, f"{ITEM}_body"),
        (string, f"{ITEM}_string"),
        (bolt, f"{ITEM}_bolt"),
    ])
    return albedo


def build_bandage(raw):
    me = raw.data
    apply_knobs(me)
    # Roll axis -> X. PCA: the roll's cylinder axis is usually the MID axis
    # (tail length is major); trust knobs + a cheap heuristic instead: put the
    # widest flat spread on X via PCA, then verify by turntable.
    pca_align(me)
    # PCA leaves the tail (major axis) on Z; the authored frame wants the roll
    # axis on X and the tail along +Y. Rotate major Z -> Y.
    me.transform(Matrix.Rotation(math.radians(-90.0), 4, "X"))
    me.update()

    # The roll is the fat end; the tail extends +Y. If the fat end (larger
    # z-extent region) sits at +Y, flip so the tail is +Y.
    _, (y0, y1), _ = bounds(me)
    h = y1 - y0

    def band_depth(lo, hi):
        zs = [v.co.z for v in me.vertices if lo <= v.co.y <= hi]
        return (max(zs) - min(zs)) if zs else 0.0

    if band_depth(y1 - 0.25 * h, y1) > band_depth(y0, y0 + 0.25 * h):
        me.transform(Matrix.Rotation(math.pi, 4, "Y"))
        me.update()

    # Fit: roll radius -> 0.10 (bottom tangent at the authored tail pivot),
    # roll center -> origin. The roll occupies the -Y end.
    _, (y0, y1), (z0, z1) = bounds(me)
    h = y1 - y0
    roll_zs = [v.co.z for v in me.vertices if v.co.y < y0 + 0.35 * h]
    s = 0.20 / (max(roll_zs) - min(roll_zs))
    me.transform(Matrix.Scale(s, 4))
    # Center the ROLL (not the whole mesh: the tail drags a centroid off) by
    # the bbox of the roll region, sized one diameter from the -Y end.
    _, (y0, y1), _ = bounds(me)
    roll = [v.co for v in me.vertices if v.co.y < y0 + 0.22]
    cx = (min(c.x for c in roll) + max(c.x for c in roll)) / 2
    cy = (min(c.y for c in roll) + max(c.y for c in roll)) / 2
    cz = (min(c.z for c in roll) + max(c.z for c in roll)) / 2
    me.transform(Matrix.Translation(Vector((-cx, -cy, -cz))))
    me.update()

    albedo = extract_albedo(raw)
    set_color(raw, (1.0, 1.0, 1.0, 1.0))

    # The reconstruction's own tail comes out as a crumpled sheet (a fringed
    # strip is beyond single-image 3D), so cut it off and DISCARD it; the
    # clean authored tail strip is reused instead. It was authored against
    # the same roll radius / pivot the fit above just reproduced, so it lines
    # up by construction and keeps its COLOR_0 on the Cloth family.
    roll_part = bisect_part(raw, Vector((0, 0.105, 0)), Vector((0, 1, 0)), False)
    bpy.data.objects.remove(raw, do_unlink=True)
    tail = import_old_prim("tail")

    assemble_and_export([
        (roll_part, f"{ITEM}_roll"),
        (tail, f"{ITEM}_tail"),
    ])
    return albedo


def main():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    raw = import_lowpoly()
    build = {"wooden_bow": build_bow, "crossbow": build_crossbow,
             "bandage": build_bandage}[ITEM]
    albedo = build(raw)
    albedo.filepath_raw = os.path.abspath(f"meshes/game/{ITEM}_albedo.png")
    albedo.file_format = "PNG"
    albedo.save()
    log(f"albedo -> meshes/game/{ITEM}_albedo.png")
    print("ANIMATABLE_DONE " + json.dumps({"item": ITEM}))


main()
