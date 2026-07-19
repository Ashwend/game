#!/usr/bin/env python3
"""Blender headless: planar-dissolve sweep over a raw TRELLIS.2 glb.

Run:
  Blender --background --python retopo_experiment.py -- <in.glb> <outdir>

The question this answers: do the faceted references fix the barrel failure?
The barrel could not be planar-dissolved (954,705 -> 489,506, curved
everywhere) and collapse-decimation shattered it. The ore references were
prompted into large flat planes precisely so that dissolve has something to
merge. This sweeps the dissolve angle and reports, per step: triangle count,
boundary/non-manifold edge counts (hole detector), and an exported glb for
turntable rendering.

Weld first: TRELLIS output has near-duplicate verts along voxel seams, and
dissolve across an unwelded seam leaves slivers. Threshold 0.2 mm on a ~1 m
prop, far below chip scale, so no detail merges.

Materials are stripped on export on purpose. The baked 2048 texture is dead
weight after dissolve (UVs shatter) and the game re-materials onto the cel
COLOR_0 system anyway; the experiment judges GEOMETRY only.

Stats print as one RETOPO_JSON line on stdout for the caller to scrape.
"""
import json
import math
import sys

import bmesh
import bpy

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
GLB = argv[0]
OUTDIR = argv[1].rstrip("/")
ANGLES_DEG = [2, 5, 8, 12]
WELD_M = 0.0002


def tri_count(obj) -> int:
    obj.data.calc_loop_triangles()
    return len(obj.data.loop_triangles)


def edge_health(obj) -> dict:
    """Boundary edges = holes; edges with >2 faces = non-manifold junk."""
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    boundary = sum(1 for e in bm.edges if len(e.link_faces) == 1)
    nonmanifold = sum(1 for e in bm.edges if len(e.link_faces) > 2)
    bm.free()
    return {"boundary_edges": boundary, "nonmanifold_edges": nonmanifold}


def main() -> None:
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=GLB)
    meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    assert meshes, "no mesh in glb"
    src = meshes[0]

    results = [{
        "variant": "raw",
        "tris": tri_count(src),
        "verts": len(src.data.vertices),
        **edge_health(src),
    }]

    for deg in ANGLES_DEG:
        obj = src.copy()
        obj.data = src.data.copy()
        bpy.context.scene.collection.objects.link(obj)
        bpy.context.view_layer.objects.active = obj
        for other in bpy.context.selected_objects:
            other.select_set(False)
        obj.select_set(True)

        w = obj.modifiers.new("weld", "WELD")
        w.merge_threshold = WELD_M
        d = obj.modifiers.new("planar", "DECIMATE")
        d.decimate_type = "DISSOLVE"
        d.angle_limit = math.radians(deg)
        d.delimit = set()
        t = obj.modifiers.new("tri", "TRIANGULATE")
        for mod in (w, d, t):
            bpy.ops.object.modifier_apply(modifier=mod.name)

        # Flat shading is the target dialect; custom split normals from the
        # TRELLIS bake would fight it and are meaningless post-dissolve.
        bpy.ops.mesh.customdata_custom_splitnormals_clear()
        obj.data.polygons.foreach_set(
            "use_smooth", [False] * len(obj.data.polygons))
        obj.data.update()

        out = f"{OUTDIR}/stone_dissolve_{deg}deg.glb"
        bpy.ops.export_scene.gltf(
            filepath=out, use_selection=True, export_materials="NONE")
        results.append({
            "variant": f"dissolve_{deg}deg",
            "tris": tri_count(obj),
            "verts": len(obj.data.vertices),
            **edge_health(obj),
            "glb": out,
        })

        obj.select_set(False)
        bpy.data.objects.remove(obj)

    print("RETOPO_JSON " + json.dumps(results))


main()
