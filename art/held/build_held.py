#!/usr/bin/env python3
"""Blender headless: TRELLIS.2 NATIVE low-poly held-item glb -> game glb + albedo.

Run (from art/held/):
  Blender --background --python build_held.py -- <item|all>

Input is the worker's native low-poly export (`gen_mesh.py --decimation-target
~10000`, saved as meshes/<key>_lowpoly.glb). This replaced the earlier
voxel-remesh -> dissolve -> collapse -> Cycles-rebake recipe entirely (owner
decision 2026-07-19 after A/B in preview.html): TRELLIS simplifies its own
reconstruction with full silhouette fidelity, while the voxel remesh eroded
anything thinner than the voxel (slimmed pick arms) and the local reduction
faceted the smooth toony surfaces. The worker's mesh, UVs, normals, and baked
texture are all kept as-is; no local resampling of any kind.

Pipeline per item:
  1. WORLD FIT: per-item knobs + PCA canonicalization into the held reference
     frame (`auto_level`: haft = major axis -> Z up, thin normal -> Y,
     wider-end-up puts the head on top, greater head reach puts the working
     edge on +X; TRELLIS output pose varies with the reference composition).
  2. TEXTURE: extract the embedded baseColor image (no bake, so the old
     metallic-bake trap cannot occur), apply the optional per-item
     ALBEDO_CURVE, save as meshes/game/<key>_albedo.png, destined for
     assets/textures/held/<key>.png.
  3. SOCKET: a `socket_grip` empty parented to the mesh at (0, 0, grip_z),
     rotated so the EXPORTED node satisfies the ART-PIPELINE-REWORK Phase 0
     contract: socket +Y along the haft toward the head, socket +Z facing the
     working edge (a +90 deg Blender-Z rotation; export maps Blender (x,y,z)
     -> glTF (x,z,-y)). The engine derives hand placement from this node.
  4. EXPORT:
     - meshes/game/<key>/model.glb: LEAN (no material; the engine attaches
       its own per-item ToonMaterial), TRELLIS UVs + normals + white COLOR_0
       (the toon shader multiplies vertex colour, so the attribute must
       exist), socket_grip node included.
     - meshes/game/review_<key>.glb: WITH the texture, for turntable/DCC
       review only (never shipped).
"""
import json
import math
import os
import sys

import bpy
from mathutils import Matrix, Vector

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
ONLY = argv[0] if argv else "all"

ITEMS = [
    "wood_stone_hatchet", "wood_stone_pickaxe",
    "iron_hatchet", "iron_pickaxe", "iron_sickle",
    # batch 2 (2026-07-19): socketless rebuilds fitted into each PREDECESSOR
    # glb's frame (measured in glTF coords, converted to Blender z-up here) so
    # every tuned legacy carry pose in held.rs keeps holding.
    "generic_held", "hammer", "building_plan", "wooden_club", "stone_spear",
    "iron_sword", "arrow", "powder_bomb", "powder_keg",
    "satchel_charge",
]

# The shared held reference frame (docs/playbooks/art-pipeline.md): pommel at
# authoring z = -0.514, head top ~ +0.356, total height ~0.87. The five
# gathering tools ship in this frame so the one socket carry fits all; the
# batch-2 items each declare their own predecessor frame via z_min.
POMMEL_Z = -0.514

# Per-item fit knobs:
#   rot_x_deg / rot_y_deg / yaw_z_deg: manual pose fixes applied BEFORE the
#     automatic canonicalization (rarely needed).
#   height: total authoring-frame height (z span after rotation).
#   width: scale by horizontal span instead of height (round items whose
#     vertical extent varies with how much fuse the model grew).
#   z_min: where the mesh bottom lands (default POMMEL_Z, the tool frame).
#   center: "grip" (haft centroid of the bottom 20% -> x=y=0, the hafted
#     default) or "full" (whole-bbox centre -> x=y=0, for bundles/props).
#   invert_head: flip the wider-end-up disambiguation for silhouettes whose
#     NARROW end is the top (sword tip, arrow tip, tied sack neck).
#   post_yaw_z_deg: yaw applied AFTER canonicalization (the hammer's head
#     must lie along Blender Y = exported glTF Z, not the PCA's X).
#   grip_z: socket_grip height on the haft (0.0 = the legacy origin).
#   socket (default True): write the socket_grip node. Batch-2 items ship
#     WITHOUT one, keeping their tuned legacy carry in held.rs.
#   trim_ground_disc: delete geometry near the pommel reaching far off the
#     haft axis (a reconstructed ground-shadow pancake). Unsafe for
#     wide-bottomed silhouettes.
#   auto_level (default True): the PCA canonicalization; set False + manual
#     rot knobs if a silhouette defeats the heuristics.
HELD_FIT = {
    "wood_stone_hatchet": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.87, grip_z=0.0),
    "wood_stone_pickaxe": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.87, grip_z=0.0),
    "iron_hatchet": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.87, grip_z=0.0),
    "iron_pickaxe": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.87, grip_z=0.0),
    "iron_sickle": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.87, grip_z=0.0),
    # ----- batch 2 (all socket=False; frames = measured predecessor glbs) -----
    # Replaces the 0.26 x 0.22 x 0.34 procedural bag cuboid (centered origin).
    "generic_held": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.32,
                         z_min=-0.16, center="full", invert_head=True, socket=False),
    # Mallet: head crossways along exported glTF Z (Blender Y).
    "hammer": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.301,
                   z_min=0.01, post_yaw_z_deg=90, socket=False),
    "building_plan": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.50,
                          z_min=-0.28, center="full", socket=False),
    "wooden_club": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.84,
                        z_min=-0.50, socket=False),
    "stone_spear": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=1.15,
                        z_min=-0.50, socket=False),
    # Tip is the NARROW end; the crossguard band is the widest.
    "iron_sword": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.88,
                       z_min=-0.53, invert_head=True, socket=False),
    # Tip up. After PCA the fletching vanes span the THIN axis, so the tip
    # band measures wider and the default wider-end-up already lands tip-up.
    "arrow": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.716,
                  z_min=-0.366, socket=False),
    # Round: PCA is unstable, scale by ball width; origin at the ball bottom
    # (the projectile renderer sinks the mesh by exactly that convention).
    "powder_bomb": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, width=0.22,
                        z_min=0.0, center="full", auto_level=False, socket=False),
    # Near-symmetric barrel: force the thin fuse end UP.
    "powder_keg": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.758,
                       z_min=0.0, center="full", invert_head=True, socket=False),
    # The gathered drawstring neck is the NARROW top.
    "satchel_charge": dict(rot_x_deg=0, rot_y_deg=0, yaw_z_deg=0, height=0.489,
                           z_min=0.0, center="full", invert_head=True, socket=False),
}

# Per-item post-extract albedo curve, in linear:
#   out = clip((in^gamma * gain) * tint + lift * exp(-L / lift_scale))
# where L is the pixel's linear luminance. Mostly empty: the native TRELLIS
# baseColor is the model's own texture work, untouched. Add a floor lift only
# if a texture truly lands near zero in-hand; `tint` is a per-channel
# multiply for hue drift (the bundle reconstructed paper-white where the
# picked burlap icon is warm grey-tan).
ALBEDO_CURVE = {
    "generic_held": dict(tint=(0.93, 0.82, 0.64)),
}


def log(msg):
    print(f"[build_held] {msg}", flush=True)


def tri_count(obj):
    obj.data.calc_loop_triangles()
    return len(obj.data.loop_triangles)


def solo_select(obj):
    for other in bpy.context.selected_objects:
        other.select_set(False)
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj


def auto_level(me, invert_head=False):
    """Canonicalize a reconstruction into the authoring frame by PCA.

    The haft is the major principal axis (-> Blender Z, up), the blade
    plane's thin normal is the minor axis (-> Y, depth), the blade spread is
    the middle axis (-> X). Then two discrete disambiguations: the head end
    is WIDER than the pommel end (else 180 about Y; `invert_head` flips this
    for narrow-tipped silhouettes like the sword and arrow), and the working
    edge is the side reaching farther from the haft in the head band (else
    180 about Z, so the edge faces +X).
    """
    import numpy as np

    pts = np.array([(v.co.x, v.co.y, v.co.z) for v in me.vertices], dtype=np.float64)
    pts -= pts.mean(axis=0)
    cov = pts.T @ pts / len(pts)
    evals, evecs = np.linalg.eigh(cov)  # ascending: minor, mid, major
    v_minor, v_mid, v_major = evecs[:, 0], evecs[:, 1], evecs[:, 2]
    rows = np.stack([v_mid, v_minor, v_major])  # maps -> (x, y, z)
    if np.linalg.det(rows) < 0:
        rows[1] = -rows[1]
    me.transform(Matrix(np.vstack([np.hstack([rows, np.zeros((3, 1))]),
                                   [0, 0, 0, 1]]).tolist()))

    def z_band(lo_frac, hi_frac):
        zs = [v.co.z for v in me.vertices]
        z0, z1 = min(zs), max(zs)
        lo = z0 + lo_frac * (z1 - z0)
        hi = z0 + hi_frac * (z1 - z0)
        return [v.co for v in me.vertices if lo <= v.co.z <= hi]

    def width(band):
        xs = [c.x for c in band]
        return (max(xs) - min(xs)) if xs else 0.0

    bottom_wider = width(z_band(0.0, 0.3)) > width(z_band(0.7, 1.0))
    if bottom_wider != invert_head:
        me.transform(Matrix.Rotation(math.pi, 4, "Y"))

    haft = z_band(0.0, 0.2)
    hx = sum(c.x for c in haft) / max(len(haft), 1)
    head = z_band(0.7, 1.0)
    reach_pos = max((c.x - hx) for c in head)
    reach_neg = max((hx - c.x) for c in head)
    if reach_neg > reach_pos:
        me.transform(Matrix.Rotation(math.pi, 4, "Z"))
    me.update()


def align_limb_down(me):
    """Rotate so the mesh's one protruding limb points straight DOWN (-Z).

    For a big-head + short-handle silhouette (the mace) PCA's major axis cuts
    diagonally through head and handle, so instead: the vertex-mass centroid
    sits inside the head, the farthest vertex from it is the handle tip, and
    the rotation taking that direction onto -Z stands the item up.
    """
    import numpy as np

    pts = np.array([(v.co.x, v.co.y, v.co.z) for v in me.vertices], dtype=np.float64)
    c = pts.mean(axis=0)
    d = pts[((pts - c) ** 2).sum(axis=1).argmax()] - c
    d /= np.linalg.norm(d)
    target = np.array([0.0, 0.0, -1.0])
    axis = np.cross(d, target)
    s = np.linalg.norm(axis)
    if s < 1e-6:
        if d[2] > 0:  # limb points straight up: flip
            me.transform(Matrix.Rotation(math.pi, 4, "X"))
        return
    angle = math.atan2(s, float(np.dot(d, target)))
    me.transform(Matrix.Rotation(angle, 4, Vector(axis / s)))
    me.update()


def world_fit(me, fit):
    """Rotate/scale/translate mesh data into the item's target frame.

    Blender axes (importer converts glTF Y-up to Z-up): up along +Z, head
    at the top, working edge toward +X. The five tools target the shared
    held reference frame; batch-2 items target their measured predecessor
    frame via `z_min` / `center` / `width`.
    """
    for axis, key in (("X", "rot_x_deg"), ("Y", "rot_y_deg"), ("Z", "yaw_z_deg")):
        if fit[key]:
            me.transform(Matrix.Rotation(math.radians(fit[key]), 4, axis))
    if fit.get("align_limb_down"):
        align_limb_down(me)
    if fit.get("auto_level", True):
        auto_level(me, invert_head=fit.get("invert_head", False))
    if fit.get("post_yaw_z_deg"):
        me.transform(Matrix.Rotation(math.radians(fit["post_yaw_z_deg"]), 4, "Z"))

    if "width" in fit:
        # Scale by the widest horizontal span (round props whose height
        # depends on how much fuse/tail the reconstruction grew).
        xs = [v.co.x for v in me.vertices]
        ys = [v.co.y for v in me.vertices]
        span = max(max(xs) - min(xs), max(ys) - min(ys))
        s = fit["width"] / span
    else:
        zs = [v.co.z for v in me.vertices]
        s = fit["height"] / (max(zs) - min(zs))
    me.transform(Matrix.Scale(s, 4))

    # Center the up axis at x=y=0 (grip centroid for hafted items, whole
    # bbox for bundles/props), then drop the bottom onto the target z_min.
    zs = [v.co.z for v in me.vertices]
    z_min = min(zs)
    if fit.get("center", "grip") == "full":
        xs = [v.co.x for v in me.vertices]
        ys = [v.co.y for v in me.vertices]
        cx = (max(xs) + min(xs)) / 2.0
        cy = (max(ys) + min(ys)) / 2.0
    else:
        z_cut = z_min + 0.2 * (max(zs) - z_min)
        grip = [v.co for v in me.vertices if v.co.z <= z_cut]
        cx = sum(c.x for c in grip) / len(grip)
        cy = sum(c.y for c in grip) / len(grip)
    target_z_min = fit.get("z_min", POMMEL_Z)
    me.transform(Matrix.Translation(Vector((-cx, -cy, target_z_min - z_min))))
    me.update()


def trim_ground_disc(me, height):
    """Delete reconstructed ground-shadow geometry near the pommel."""
    import bmesh
    z_cut = POMMEL_Z + 0.06 * height
    bm = bmesh.new()
    bm.from_mesh(me)
    doomed = [
        v for v in bm.verts
        if v.co.z <= z_cut and (v.co.x * v.co.x + v.co.y * v.co.y) > 0.09 * 0.09
    ]
    if doomed:
        bmesh.ops.delete(bm, geom=doomed, context="VERTS")
        bm.to_mesh(me)
        me.update()
        log(f"  trimmed ground disc: {len(doomed)} verts")
    bm.free()


def extract_base_color(obj, key):
    """The image feeding the Principled Base Color input (TRELLIS's own baked
    albedo). Extracting instead of re-baking is what makes the old
    metallic-bake trap impossible on this path."""
    for mat in obj.data.materials:
        if not mat or not mat.use_nodes:
            continue
        for node in mat.node_tree.nodes:
            if node.type != "BSDF_PRINCIPLED":
                continue
            for link in node.inputs["Base Color"].links:
                upstream = link.from_node
                if upstream.type == "TEX_IMAGE" and upstream.image:
                    return upstream.image
    # Fallback: the largest packed image.
    images = [i for i in bpy.data.images if i.size[0] > 0]
    assert images, f"{key}: no image found in glb"
    return max(images, key=lambda i: i.size[0] * i.size[1])


def apply_albedo_curve(img, curve):
    import numpy as np
    px = np.empty(len(img.pixels), dtype=np.float32)
    img.pixels.foreach_get(px)
    px = px.reshape(-1, 4)
    rgb = np.clip(px[:, :3], 0.0, 1.0)
    rgb = rgb ** curve.get("gamma", 1.0) * curve.get("gain", 1.0)
    tint = curve.get("tint")
    if tint:
        rgb *= np.asarray(tint, dtype=np.float32)
    lift = curve.get("lift", 0.0)
    if lift:
        lum = rgb[:, 0] * 0.2126 + rgb[:, 1] * 0.7152 + rgb[:, 2] * 0.0722
        rgb += (lift * np.exp(-lum / curve["lift_scale"]))[:, None]
    px[:, :3] = np.clip(rgb, 0.0, 1.0)
    img.pixels.foreach_set(px.reshape(-1).tolist())


def add_grip_socket(obj, grip_z):
    """socket_grip empty, child of the mesh, per the Phase 0 contract."""
    sock = bpy.data.objects.new("socket_grip", None)
    sock.empty_display_type = "ARROWS"
    sock.empty_display_size = 0.05
    bpy.context.scene.collection.objects.link(sock)
    sock.parent = obj
    sock.location = Vector((0.0, 0.0, grip_z))
    sock.rotation_euler = (0.0, 0.0, math.radians(90.0))
    return sock


def export_glb(objs, path, materials):
    solo_select(objs[0])
    for o in objs[1:]:
        o.select_set(True)
    kwargs = dict(filepath=path, use_selection=True, export_materials=materials)
    try:
        bpy.ops.export_scene.gltf(**kwargs, export_vertex_color="ACTIVE")
    except TypeError:
        bpy.ops.export_scene.gltf(**kwargs)


def build_one(key):
    src_path = f"meshes/{key}_lowpoly.glb"
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=src_path)
    meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    assert meshes, f"no mesh in {src_path}"
    obj = meshes[0]
    obj.parent = None
    obj.matrix_world = Matrix.Identity(4)
    tris = tri_count(obj)

    fit = HELD_FIT[key]
    world_fit(obj.data, fit)
    if fit.get("trim_ground_disc"):
        trim_ground_disc(obj.data, fit["height"])

    img = extract_base_color(obj, key)
    curve = ALBEDO_CURVE.get(key)
    if curve:
        apply_albedo_curve(img, curve)

    me = obj.data
    # The toon path multiplies COLOR_0 into the texture; ship it white so the
    # extracted albedo passes through unchanged.
    attr = me.color_attributes.new("Color", "FLOAT_COLOR", "CORNER")
    attr.data.foreach_set("color", [1.0] * (len(attr.data) * 4))

    exported = [obj]
    if fit.get("socket", True):
        exported.append(add_grip_socket(obj, fit.get("grip_z", 0.0)))

    os.makedirs(f"meshes/game/{key}", exist_ok=True)
    img.filepath_raw = os.path.abspath(f"meshes/game/{key}_albedo.png")
    img.file_format = "PNG"
    img.save()

    export_glb(exported, f"meshes/game/review_{key}.glb", "EXPORT")
    obj.data.materials.clear()
    export_glb(exported, f"meshes/game/{key}/model.glb", "NONE")

    zs = [round(f(v.co.z for v in me.vertices), 3) for f in (min, max)]
    xs = [round(f(v.co.x for v in me.vertices), 3) for f in (min, max)]
    return {
        "item": key, "tris": tris, "z_range": zs, "x_range": xs,
        "albedo": f"meshes/game/{key}_albedo.png",
        "model": f"meshes/game/{key}/model.glb",
    }


def main():
    os.makedirs("meshes/game", exist_ok=True)
    keys = ITEMS if ONLY == "all" else [ONLY]
    results = [build_one(k) for k in keys]
    print("BUILD_JSON " + json.dumps(results))


main()
