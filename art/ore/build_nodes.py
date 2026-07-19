#!/usr/bin/env python3
"""Blender headless: raw TRELLIS.2 ore glb -> game-format low-poly glb + albedo.

Run:
  Blender --background --python build_nodes.py -- <type|all> [angle_deg]
      [target_tris] [voxel_m] [bake_px]

Pipeline per node type (input meshes/<type>.glb):
  1. GEOMETRY: voxel remesh (one watertight skin; the raw mesh's ~5k
     non-manifold chunk-intersection edges both stall and mangle collapse
     decimation) -> planar dissolve -> triangulate -> gentle collapse to the
     triangle budget -> flat shading.
  2. UV: Smart UV Project on the low-poly. Overlap-free islands, which the
     bake step requires (the earlier box projection overlapped front/back).
  3. TEXTURE: Cycles selected-to-active DIFFUSE/COLOR bake, raw mesh ->
     low-poly. This carries the TRELLIS-baked albedo (the AI model's texture
     work) onto the game mesh instead of discarding it. Runs BEFORE the
     world-fit transform because the bake matches surfaces in world space.
  4. WORLD FIT: per-type up-axis fix (TRELLIS keeps the reference IMAGE's
     up-axis; the meteorite arrived standing on end), ground at z=0, uniform
     scale to the shipped footprint (~1.26 m, 1.34 meteorite).
  5. EXPORT:
     - meshes/game/<type>/stage_{0,1,2}.glb: LEAN (no material; the engine
       attaches its own per-type ToonMaterial), UVs + white COLOR_0 (the
       toon shader multiplies vertex color, so the attribute must exist).
       Stage 1/2 are scaled copies (shipped ratios), sharing UVs + texture.
     - meshes/game/<type>_albedo.png: the baked texture, destined for
       assets/textures/ore/<type>.png.
     - meshes/game/review_<type>.glb: stage_0 WITH the texture embedded, for
       turntable/DCC review only (never shipped).
"""
import json
import math
import os
import sys

import bpy
from mathutils import Matrix, Vector

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
ONLY = argv[0] if argv else "all"
ANGLE_DEG = float(argv[1]) if len(argv) > 1 else 12.0
TARGET_TRIS = int(argv[2]) if len(argv) > 2 else 1500
VOXEL_M = float(argv[3]) if len(argv) > 3 else 0.015
BAKE_PX = int(argv[4]) if len(argv) > 4 else 1024

TYPES = ["stone", "iron", "coal", "sulfur", "meteorite"]

# World-fit per type. rot_x_deg lays a mesh down when TRELLIS kept the
# reference image's up-axis; footprint matches the shipped glbs the gameplay
# tuning was built around.
WORLD_FIT = {
    "stone": dict(rot_x_deg=0, footprint=1.26),
    "iron": dict(rot_x_deg=0, footprint=1.26),
    "coal": dict(rot_x_deg=0, footprint=1.26),
    "sulfur": dict(rot_x_deg=0, footprint=1.26),
    "meteorite": dict(rot_x_deg=-90, footprint=1.34),
}
# Depletion-stage scale ratios (footprint, height), measured from the shipped
# stage glbs: worn-down keeps most of the footprint but loses height.
STAGE_RATIOS = [(1.0, 1.0), (0.92, 0.72), (0.85, 0.51)]

# Per-type post-bake albedo curve, out = clip(in^gamma * gain), in linear.
# The meteorite reference is near-black (body ~0.02 linear), and baked
# verbatim the node renders as a silhouette hole in-world (zero surface
# readability under the cel shade bands). A flat gain cannot lift values
# that close to zero without clipping the nuggets, so use a gamma lift:
# 0.55 takes the body to a readable dark slag (~0.12 linear) while barely
# moving the pale nuggets.
ALBEDO_CURVE = {"meteorite": dict(gamma=0.4, gain=1.1)}


def log(msg):
    print(f"[build_nodes] {msg}", flush=True)


def tri_count(obj):
    obj.data.calc_loop_triangles()
    return len(obj.data.loop_triangles)


def solo_select(obj):
    for other in bpy.context.selected_objects:
        other.select_set(False)
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj


def bake_albedo(src, dst, kind):
    """Cycles selected-to-active DIFFUSE color bake, src -> dst's UV image.

    cage_extrusion covers the low-poly's deviation from the raw surface
    (voxel size + collapse error, a few cm); rays that miss entirely leave
    the margin fill, which the dissolve-flattened surface makes rare.
    """
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = 16

    img = bpy.data.images.new(f"{kind}_albedo", BAKE_PX, BAKE_PX, alpha=False)
    mat = bpy.data.materials.new(f"{kind}_baked")
    mat.use_nodes = True
    tree = mat.node_tree
    tex = tree.nodes.new("ShaderNodeTexImage")
    tex.image = img
    tree.nodes.active = tex
    tree.links.new(tex.outputs["Color"],
                   tree.nodes["Principled BSDF"].inputs["Base Color"])
    tree.nodes["Principled BSDF"].inputs["Roughness"].default_value = 0.9
    dst.data.materials.clear()
    dst.data.materials.append(mat)

    solo_select(dst)
    src.select_set(True)
    bpy.ops.object.bake(
        type="DIFFUSE",
        pass_filter={"COLOR"},
        use_selected_to_active=True,
        cage_extrusion=0.08,
        max_ray_distance=0.3,
        margin=8,
        use_clear=True,
    )
    return img


def build_one(kind):
    src_path = f"meshes/{kind}.glb"
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=src_path)
    meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    assert meshes, f"no mesh in {src_path}"
    src = meshes[0]
    raw_tris = tri_count(src)

    obj = src.copy()
    obj.data = src.data.copy()
    bpy.context.scene.collection.objects.link(obj)
    solo_select(obj)

    r = obj.modifiers.new("remesh", "REMESH")
    r.mode = "VOXEL"
    r.voxel_size = VOXEL_M
    d = obj.modifiers.new("planar", "DECIMATE")
    d.decimate_type = "DISSOLVE"
    d.angle_limit = math.radians(ANGLE_DEG)
    d.delimit = set()
    t = obj.modifiers.new("tri", "TRIANGULATE")
    for mod in (r, d, t):
        bpy.ops.object.modifier_apply(modifier=mod.name)
    dissolved = tri_count(obj)
    if dissolved > TARGET_TRIS:
        c = obj.modifiers.new("collapse", "DECIMATE")
        c.decimate_type = "COLLAPSE"
        c.ratio = TARGET_TRIS / dissolved
        bpy.ops.object.modifier_apply(modifier=c.name)
    final = tri_count(obj)

    bpy.ops.mesh.customdata_custom_splitnormals_clear()
    obj.data.polygons.foreach_set("use_smooth", [False] * len(obj.data.polygons))
    obj.data.update()

    for uvl in list(obj.data.uv_layers):
        obj.data.uv_layers.remove(uvl)
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.uv.smart_project(angle_limit=math.radians(66), island_margin=0.01)
    bpy.ops.object.mode_set(mode="OBJECT")

    log(f"{kind}: baking {BAKE_PX}px albedo from {raw_tris:,} raw tris...")
    img = bake_albedo(src, obj, kind)
    curve = ALBEDO_CURVE.get(kind)
    if curve:
        import numpy as np
        px = np.empty(len(img.pixels), dtype=np.float32)
        img.pixels.foreach_get(px)
        px = px.reshape(-1, 4)
        rgb = np.clip(px[:, :3], 0.0, 1.0)
        px[:, :3] = np.clip(rgb ** curve["gamma"] * curve["gain"], 0.0, 1.0)
        img.pixels.foreach_set(px.reshape(-1).tolist())

    src.select_set(False)
    bpy.data.objects.remove(src)

    # World fit, in BLENDER axes (importer converts glTF Y-up to Z-up, so
    # height here is Z and the footprint is XY).
    fit = WORLD_FIT[kind]
    me = obj.data
    if fit["rot_x_deg"]:
        me.transform(Matrix.Rotation(math.radians(fit["rot_x_deg"]), 4, "X"))
    xs = [v.co.x for v in me.vertices]
    ys = [v.co.y for v in me.vertices]
    zs = [v.co.z for v in me.vertices]
    span = max(max(xs) - min(xs), max(ys) - min(ys))
    s = fit["footprint"] / span
    center = Vector(((max(xs) + min(xs)) / 2, (max(ys) + min(ys)) / 2, min(zs)))
    me.transform(Matrix.Translation(-center))
    me.transform(Matrix.Scale(s, 4))
    me.update()

    # The toon shader multiplies COLOR_0 into the texture; ship it white so
    # the baked albedo passes through unchanged.
    attr = me.color_attributes.new("Color", "FLOAT_COLOR", "CORNER")
    white = [1.0] * (len(attr.data) * 4)
    attr.data.foreach_set("color", white)

    os.makedirs(f"meshes/game/{kind}", exist_ok=True)
    img.filepath_raw = os.path.abspath(f"meshes/game/{kind}_albedo.png")
    img.file_format = "PNG"
    img.save()

    # Review copy first (with the baked material), then the lean stages.
    solo_select(obj)
    review = f"meshes/game/review_{kind}.glb"
    kwargs = dict(filepath=review, use_selection=True, export_materials="EXPORT")
    try:
        bpy.ops.export_scene.gltf(**kwargs, export_vertex_color="ACTIVE")
    except TypeError:
        bpy.ops.export_scene.gltf(**kwargs)

    stage_paths = []
    for stage, (fp_r, h_r) in enumerate(STAGE_RATIOS):
        st = obj.copy()
        st.data = obj.data.copy()
        st.data.materials.clear()
        bpy.context.scene.collection.objects.link(st)
        st.data.transform(Matrix.Diagonal((fp_r, fp_r, h_r, 1.0)))
        st.data.update()
        solo_select(st)
        path = f"meshes/game/{kind}/stage_{stage}.glb"
        kwargs = dict(filepath=path, use_selection=True, export_materials="NONE")
        try:
            bpy.ops.export_scene.gltf(**kwargs, export_vertex_color="ACTIVE")
        except TypeError:
            bpy.ops.export_scene.gltf(**kwargs)
        stage_paths.append(path)
        st.select_set(False)
        bpy.data.objects.remove(st)

    return {
        "type": kind, "raw_tris": raw_tris, "dissolved_tris": dissolved,
        "final_tris": final, "albedo": f"meshes/game/{kind}_albedo.png",
        "stages": stage_paths,
    }


def main():
    os.makedirs("meshes/game", exist_ok=True)
    kinds = TYPES if ONLY == "all" else [ONLY]
    results = [build_one(k) for k in kinds]
    print("BUILD_JSON " + json.dumps(results))


main()
