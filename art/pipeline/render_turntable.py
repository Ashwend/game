#!/usr/bin/env python3
"""Blender headless: load a .glb, render an N-angle turntable strip, report stats.

Run:
  Blender --background --python render_turntable.py -- <in.glb> <out.png> [angles] [px]

The strip is what makes an image-to-3D result judgeable. A single hero angle
hides exactly the failures this pipeline produces: an unresolved back side, a
silhouette that only reads from the reference camera, and holes left by
decimation. Four angles at 90 degrees show all of it.

Stats are printed as one STATS_JSON line on stdout so the caller can scrape them
without parsing Blender's chatter. Triangle count is the number that decides
whether a mesh is shippable: the game's authored ore glbs are a few hundred
triangles, and raw TRELLIS.2 output has been measured at 954,705.
"""
import json
import math
import sys

import bpy

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
GLB = argv[0]
OUT = argv[1] if len(argv) > 1 else "/tmp/turntable.png"
ANGLES = int(argv[2]) if len(argv) > 2 else 4
PX = int(argv[3]) if len(argv) > 3 else 512
# "color" mode: Standard view transform + gentler lights. The default rig
# (hot key + AgX) is right for judging silhouette on white raw meshes but
# desaturates palette colors into wash; use this mode when judging COLOR_0.
COLOR_MODE = len(argv) > 4 and argv[4] == "color"


def clear_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)


def import_glb(path):
    bpy.ops.import_scene.gltf(filepath=path)
    return [o for o in bpy.context.scene.objects if o.type == "MESH"]


def bounds(meshes):
    """World-space AABB over every imported mesh."""
    lo = [float("inf")] * 3
    hi = [float("-inf")] * 3
    for obj in meshes:
        for corner in obj.bound_box:
            world = obj.matrix_world @ __import__("mathutils").Vector(corner)
            for i in range(3):
                lo[i] = min(lo[i], world[i])
                hi[i] = max(hi[i], world[i])
    return lo, hi


def setup_lighting():
    """Neutral three-point-ish rig. Deliberately soft and even: we are judging
    SILHOUETTE and FACETING, so dramatic lighting would flatter a bad mesh."""
    key = bpy.data.lights.new("key", type="AREA")
    # Sized for a roughly 1 m prop viewed from ~2.4 m. The first pass at 400 W
    # rendered near-black on a dark material, which would have made a bad mesh
    # look like a lighting problem and vice versa.
    key.energy = 900 if COLOR_MODE else 2500
    key.size = 4
    key_obj = bpy.data.objects.new("key", key)
    key_obj.location = (3, -3, 4)
    key_obj.rotation_euler = (math.radians(45), 0, math.radians(45))
    bpy.context.scene.collection.objects.link(key_obj)

    fill = bpy.data.lights.new("fill", type="AREA")
    fill.energy = 350 if COLOR_MODE else 900
    fill.size = 6
    fill_obj = bpy.data.objects.new("fill", fill)
    fill_obj.location = (-4, -2, 2)
    fill_obj.rotation_euler = (math.radians(70), 0, math.radians(-60))
    bpy.context.scene.collection.objects.link(fill_obj)

    world = bpy.data.worlds.new("world")
    world.use_nodes = True
    # Dark backdrop: these props are light grey granite, so a mid-grey world
    # would sit at the same value as the subject and hide the silhouette.
    world.node_tree.nodes["Background"].inputs[0].default_value = (0.055, 0.06, 0.07, 1)
    world.node_tree.nodes["Background"].inputs[1].default_value = 1.0
    bpy.context.scene.world = world


def main():
    clear_scene()
    meshes = import_glb(GLB)
    if not meshes:
        print("STATS_JSON " + json.dumps({"error": "no mesh in glb"}))
        return

    tris = 0
    verts = 0
    for obj in meshes:
        obj.data.calc_loop_triangles()
        tris += len(obj.data.loop_triangles)
        verts += len(obj.data.vertices)

    lo, hi = bounds(meshes)
    center = [(lo[i] + hi[i]) / 2 for i in range(3)]
    size = max(hi[i] - lo[i] for i in range(3)) or 1.0

    setup_lighting()

    target = bpy.data.objects.new("target", None)
    target.location = center
    bpy.context.scene.collection.objects.link(target)

    cam_data = bpy.data.cameras.new("cam")
    cam_data.lens = 50
    cam = bpy.data.objects.new("cam", cam_data)
    bpy.context.scene.collection.objects.link(cam)
    bpy.context.scene.camera = cam
    track = cam.constraints.new("TRACK_TO")
    track.target = target
    track.track_axis = "TRACK_NEGATIVE_Z"
    track.up_axis = "UP_Y"

    scene = bpy.context.scene
    # EEVEE was renamed BLENDER_EEVEE_NEXT in Blender 4.2. Pick whichever this
    # build actually offers rather than pinning a name that breaks on the other.
    engines = scene.render.bl_rna.properties["engine"].enum_items.keys()
    scene.render.engine = next(
        (e for e in ("BLENDER_EEVEE_NEXT", "BLENDER_EEVEE") if e in engines),
        "BLENDER_WORKBENCH",
    )
    scene.render.resolution_x = PX
    scene.render.resolution_y = PX
    if COLOR_MODE:
        scene.view_settings.view_transform = "Standard"
    scene.render.film_transparent = False
    scene.render.image_settings.file_format = "PNG"

    # Slightly above the horizon, matching how the player sees a knee-high
    # deposit while walking up to it.
    dist = size * 2.4
    elev = math.radians(22)
    frames = []
    for i in range(ANGLES):
        az = 2 * math.pi * i / ANGLES
        cam.location = (
            center[0] + dist * math.cos(az) * math.cos(elev),
            center[1] + dist * math.sin(az) * math.cos(elev),
            center[2] + dist * math.sin(elev),
        )
        path = f"{OUT}.frame{i}.png"
        scene.render.filepath = path
        bpy.ops.render.render(write_still=True)
        frames.append(path)

    print("STATS_JSON " + json.dumps({
        "tris": tris, "verts": verts, "objects": len(meshes),
        "size_m": [round(hi[i] - lo[i], 3) for i in range(3)],
        "frames": frames,
    }))


main()
