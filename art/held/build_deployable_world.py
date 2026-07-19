#!/usr/bin/env python3
"""Blender headless: TRELLIS lowpoly (~3k tris) -> deployable WORLD glbs.

Run (from art/held/):  Blender --background --python build_deployable_world.py -- <key|all>

World-placed props from the batch-2 icon picks, fitted into each OLD world
glb's measured frame (base at y=0, footprint centered; doors hinge at the
x=0 edge and are scaled EXACTLY to the old panel width/height so they keep
filling their openings). TRELLIS's upright canonical pose is trusted (props
photograph upright); per-item rot knobs correct the exceptions, verified by
turntable. White COLOR_0 + extracted albedo (the placed material becomes the
per-item baked ToonMaterial). The wood_shutter keeps its procedural panel for
now (its world mesh is not a glb).
"""
import json
import math
import os
import sys

import bpy
from mathutils import Matrix, Vector

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
ONLY = argv[0] if argv else "all"

# Old-world frames (glTF coords). height = y span; doors carry exact w/h.
FIT = {
    "workbench_t1": dict(height=0.835),
    "workbench_t2": dict(height=1.028),
    "crude_furnace": dict(height=1.096),
    "hewn_log_door": dict(door_w=1.04, door_h=2.14),
    "iron_door": dict(door_w=1.04, door_h=2.14),
    "sleeping_bag": dict(width=1.93),
    "storage_box_small": dict(height=0.688),
    "storage_box_large": dict(height=0.828),
    "torch": dict(height=0.52),
    "tool_cupboard": dict(height=1.823),
    "ruin_cache": dict(width=0.92),
}
KNOBS = {k: dict(rot_x_deg=0, rot_y_deg=0, rot_z_deg=0) for k in FIT}
# The ruin cache reconstructed with its long axis on depth; yaw it square.
# (The workbench_t2 knob from the first reference is retired with it.)
KNOBS["ruin_cache"]["rot_z_deg"] = 90
# The redo chest reconstructs hasp-forward already (glTF +Z); no yaw.


def log(msg):
    print(f"[deploy_world] {msg}", flush=True)


def bounds(me):
    xs = [v.co.x for v in me.vertices]
    ys = [v.co.y for v in me.vertices]
    zs = [v.co.z for v in me.vertices]
    return (min(xs), max(xs)), (min(ys), max(ys)), (min(zs), max(zs))


def build_one(key):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=f"meshes/{key}_lowpoly.glb")
    objs = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    obj = objs[0]
    obj.parent = None
    obj.matrix_world = Matrix.Identity(4)
    me = obj.data

    k = KNOBS[key]
    for axis, kk in (("X", "rot_x_deg"), ("Y", "rot_y_deg"), ("Z", "rot_z_deg")):
        if k[kk]:
            me.transform(Matrix.Rotation(math.radians(k[kk]), 4, axis))

    fit = FIT[key]
    (x0, x1), (y0, y1), (z0, z1) = bounds(me)
    # Blender import: up = +Z (glTF y), so "height" is the Blender z span and
    # the ground plane is z. Doors: width = x, height = z, thickness = y.
    if "door_w" in fit:
        sx = fit["door_w"] / (x0 != x1 and (x1 - x0) or 1)
        sz = fit["door_h"] / (z1 - z0)
        sy = min(sx, sz)
        me.transform(Matrix.Diagonal(Vector((sx, sy, sz))).to_4x4())
        (x0, x1), (y0, y1), (z0, z1) = bounds(me)
        # Hinge edge at x=0, base at z=0, thickness centered.
        me.transform(Matrix.Translation(Vector((-x0, -(y0 + y1) / 2, -z0))))
    else:
        if "width" in fit:
            s = fit["width"] / (x1 - x0)
        else:
            s = fit["height"] / (z1 - z0)
        me.transform(Matrix.Scale(s, 4))
        (x0, x1), (y0, y1), (z0, z1) = bounds(me)
        me.transform(Matrix.Translation(Vector((
            -(x0 + x1) / 2, -(y0 + y1) / 2, -z0))))
    me.update()

    # Extract the albedo, ship white COLOR_0.
    img = None
    for mat in obj.data.materials:
        if not mat or not mat.use_nodes:
            continue
        for node in mat.node_tree.nodes:
            if node.type == "BSDF_PRINCIPLED":
                for link in node.inputs["Base Color"].links:
                    if link.from_node.type == "TEX_IMAGE" and link.from_node.image:
                        img = link.from_node.image
    if img is None:
        img = max((i for i in bpy.data.images if i.size[0] > 0),
                  key=lambda i: i.size[0] * i.size[1])
    for attr in list(me.color_attributes):
        me.color_attributes.remove(attr)
    attr = me.color_attributes.new("Color", "FLOAT_COLOR", "CORNER")
    attr.data.foreach_set("color", [1.0] * (len(attr.data) * 4))

    os.makedirs(f"meshes/game/{key}", exist_ok=True)
    img.filepath_raw = os.path.abspath(f"meshes/game/{key}_albedo.png")
    img.file_format = "PNG"
    img.save()
    obj.data.materials.clear()
    for o in bpy.data.objects:
        o.select_set(o is obj)
    bpy.context.view_layer.objects.active = obj
    kwargs = dict(filepath=f"meshes/game/{key}/model.glb", use_selection=True,
                  export_materials="NONE")
    try:
        bpy.ops.export_scene.gltf(**kwargs, export_vertex_color="ACTIVE")
    except TypeError:
        bpy.ops.export_scene.gltf(**kwargs)
    me.calc_loop_triangles()
    (x0, x1), (y0, y1), (z0, z1) = bounds(me)
    return {"item": key, "tris": len(me.loop_triangles),
            "gl_size": [round(x1 - x0, 3), round(z1 - z0, 3), round(y1 - y0, 3)]}


def main():
    keys = list(FIT) if ONLY == "all" else [ONLY]
    print("DEPLOY_BUILD " + json.dumps([build_one(key) for key in keys]))


main()
