#!/usr/bin/env python3
"""Render an item glb to a transparent 512px icon master from a fixed 3/4 camera.

Mesh-rendered icons: once an item has
a mesh anyway, rendering the icon from that mesh is consistent, repeatable, and
regenerates for free after any model tweak. This is the Blender-headless half; the
committed scripts/icon_finalize.py then downscales the master to the in-game
assets/items/<id>/icon.png.

The camera angle is FIXED so every item shares one 3/4 view (the whole point is a
consistent icon set). The glb carries COLOR_0 vertex colours and two material
slots (grip + head); this renderer wires a simple vertex-colour EEVEE material per
slot (a warmer look for the head slot on iron weapons) purely for the icon, since
the in-game material lives in Rust and is not embedded. Lighting is a key + fill +
rim so the faceted silhouette reads without blowing out.

Reusable for ANY future item: pass the glb path and the output id.

Run headless:
  Blender -b -P scripts/render_icon.py -- <glb_path> <out_master_png> [size]
  Blender -b -P scripts/render_icon.py -- assets/items/iron_sword/model.glb \
      art/items/iron_sword/icon_master_512.png
"""

import bpy
import math
import os
import sys

from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
if len(argv) < 2:
    raise SystemExit("usage: render_icon.py -- <glb> <out.png> [size]")
GLB = argv[0]
OUT = argv[1]
SIZE = int(argv[2]) if len(argv) > 2 else 512

# Fixed 3/4 view shared by every icon: azimuth around the item, a slight top-down
# tilt, framed so the whole silhouette fits with a margin. Held items stand tall
# (in-game +Y up == glTF Y == imported Blender Z), so we orbit around Z and look
# down a little.
#
# Weapons keep the default 34 deg azimuth (their hero edge faces glTF +X, into the
# view). Worn armour puts its hero detail on the FRONT (glTF -Z, the rig's forward),
# which the default camera would show from behind, so the armour build passes
# ICON_AZIMUTH_DEG (via env) to orbit round to a front-3/4. Defaults are unchanged,
# so every existing icon still renders identically.
AZIMUTH_DEG = float(os.environ.get("ICON_AZIMUTH_DEG", "34.0"))
ELEVATION_DEG = float(os.environ.get("ICON_ELEVATION_DEG", "18.0"))
FIT_MARGIN = 1.18       # camera pullback vs the bounding radius (legacy radial fit)

# Camera fit mode. The default "radial" fit (radius = half the LARGEST bound axis
# against an assumed 20 deg half-FOV) under-frames boxy shapes twice over: the real
# half-FOV of the 55mm lens is only ~18.1 deg, and a wide+deep shape's screen extent
# is set by its projected CORNERS (the box diagonal), not one axis. Long thin
# weapons fit anyway, and their output must stay byte-identical, so the exact
# corner-projection fit is opt-in:
#   ICON_FIT_MODE=corners   fit distance from all 8 projected AABB corners
#   ICON_FIT_MARGIN=1.14    breathing room for corners mode (default 14%)
# Armour renders use corners mode; weapons keep the default radial fit.
FIT_MODE = os.environ.get("ICON_FIT_MODE", "radial")
CORNER_FIT_MARGIN = float(os.environ.get("ICON_FIT_MARGIN", "1.14"))

# Light scaling. The shared key/fill/rim energies were tuned on thin weapon
# silhouettes; large smooth armour shells present whole planes to the key sun and
# blow out toward white, and the view transform then desaturates the highlights,
# killing the COLOR_0 identity (warm cloth turns grey-beige, steel turns paper-white
# and the dark rivet dots vanish). ICON_LIGHT_SCALE multiplies the three sun
# energies and ICON_WORLD_STRENGTH sets the ambient fill strength; the defaults
# leave every existing (weapon) icon byte-identical.
LIGHT_SCALE = float(os.environ.get("ICON_LIGHT_SCALE", "1.0"))
WORLD_STRENGTH = float(os.environ.get("ICON_WORLD_STRENGTH", "0.55"))

# View transform. Blender's default (AgX in 4.x/5.x) desaturates and lifts
# mid-to-bright tones: warm tan cloth renders grey-beige and bright steel goes
# paper-white, erasing dark rivet dots. Weapons were tuned under the default and
# keep it; armour renders pass ICON_VIEW_TRANSFORM=Standard (a direct linear->sRGB
# encode) so the COLOR_0 set identity survives, with ICON_LIGHT_SCALE lowered to
# keep bright steel from clipping.
VIEW_TRANSFORM = os.environ.get("ICON_VIEW_TRANSFORM", "")  # ""=Blender default

# Icon-only material brightness. Iron armour COLOR_0 is bright on purpose (on the
# in-game metal slot it drives F0, not diffuse), so under a Standard transform the
# plates render near-white at ANY sane light level and the dark rivet dots lose
# contrast. ICON_COLOR_GAIN multiplies the vertex colour in the icon material only
# (the glb is untouched); 1.0 (default) inserts no node, so existing renders are
# pixel-identical. The engine's in-game look is unaffected either way.
COLOR_GAIN = float(os.environ.get("ICON_COLOR_GAIN", "1.0"))


def vertex_color_material(name, warm):
    """A flat-ish EEVEE material reading COLOR_0, tinted marginally warm for metal
    heads so the icon does not look grey-dead. Icon only; not the in-game shader."""
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nt = m.node_tree
    bsdf = nt.nodes.get("Principled BSDF")
    bsdf.inputs["Roughness"].default_value = 0.5 if warm else 0.8
    bsdf.inputs["Metallic"].default_value = 0.25 if warm else 0.0
    bsdf.inputs["Specular IOR Level"].default_value = 0.3 if warm else 0.15
    vc = nt.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    if COLOR_GAIN != 1.0:
        mul = nt.nodes.new("ShaderNodeMixRGB")
        mul.blend_type = 'MULTIPLY'
        mul.inputs["Factor"].default_value = 1.0
        g = COLOR_GAIN
        mul.inputs["Color2"].default_value = (g, g, g, 1.0)
        nt.links.new(vc.outputs["Color"], mul.inputs["Color1"])
        nt.links.new(mul.outputs["Color"], bsdf.inputs["Base Color"])
    else:
        nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    m.use_backface_culling = True
    return m


def main():
    bpy.ops.wm.read_homefile(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=GLB)
    meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    if not meshes:
        raise SystemExit(f"no mesh in {GLB}")

    # Reassign an icon material per slot (slot 0 grip = matte, slot 1 head = warm).
    # Only the iron WEAPON heads get the warm semi-metallic treatment: it flatters
    # small faceted blades but puts a white specular floor under large smooth
    # armour plates (the iron armour renders matte + ICON_COLOR_GAIN instead).
    is_metal = any(k in os.path.basename(os.path.dirname(GLB))
                   for k in ("iron_sword", "iron_mace"))
    grip_mat = vertex_color_material("icon_grip", warm=False)
    head_mat = vertex_color_material("icon_head", warm=is_metal)
    for o in meshes:
        me = o.data
        me.materials.clear()
        me.materials.append(grip_mat)
        me.materials.append(head_mat)
        # material_index survives import; if a slot went missing, clamp to 0.
        for p in me.polygons:
            if p.material_index > 1:
                p.material_index = 0

    # World bounds + centre.
    mn = [1e9] * 3
    mx = [-1e9] * 3
    for o in meshes:
        for v in o.data.vertices:
            co = o.matrix_world @ v.co
            for i in range(3):
                mn[i] = min(mn[i], co[i])
                mx[i] = max(mx[i], co[i])
    center = Vector(((mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2, (mn[2] + mx[2]) / 2))
    radius = max((mx[i] - mn[i]) for i in range(3)) * 0.5

    scene = bpy.context.scene
    scene.render.engine = 'BLENDER_EEVEE'
    scene.render.film_transparent = True
    scene.render.resolution_x = SIZE
    scene.render.resolution_y = SIZE
    scene.render.image_settings.file_format = 'PNG'
    scene.render.image_settings.color_mode = 'RGBA'
    if VIEW_TRANSFORM:
        scene.view_settings.view_transform = VIEW_TRANSFORM

    # World: soft neutral fill so shadowed facets keep some colour.
    world = bpy.data.worlds.new("w")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs[0].default_value = (0.55, 0.57, 0.60, 1)
    world.node_tree.nodes["Background"].inputs[1].default_value = WORLD_STRENGTH
    scene.world = world

    # Camera: fixed 3/4. Orbit position from azimuth + elevation around center.
    az = math.radians(AZIMUTH_DEG)
    el = math.radians(ELEVATION_DEG)
    cam_dir = Vector((math.cos(el) * math.cos(az),
                      math.cos(el) * math.sin(az),
                      math.sin(el)))
    cam_data = bpy.data.cameras.new("cam")
    cam_data.lens = 55

    if FIT_MODE == "corners":
        # Exact fit: for each of the 8 AABB corners, solve the minimum camera
        # distance that keeps its projection inside the real FOV (with margin),
        # and take the worst case. Camera basis: forward f from camera to centre,
        # right/up from the world-Z track (matches to_track_quat('-Z','Y') for a
        # non-vertical f). Requirement per corner offset o = corner - center:
        #   |o . right| <= tan(half_fov) * (dist + o . f) / margin   (and same for up)
        # => dist >= margin * |o . axis| / tan(half_fov) - o . f.
        f = -cam_dir  # camera looks along f toward the centre
        right = f.cross(Vector((0, 0, 1))).normalized()
        up = right.cross(f).normalized()
        # cam_data.angle is the full FOV of the sensor's larger dimension; the
        # render is square so horizontal == vertical.
        half_t = math.tan(cam_data.angle / 2)
        dist = 0.0
        for sx in (mn[0], mx[0]):
            for sy in (mn[1], mx[1]):
                for sz in (mn[2], mx[2]):
                    o = Vector((sx, sy, sz)) - center
                    for axis in (right, up):
                        need = CORNER_FIT_MARGIN * abs(o.dot(axis)) / half_t - o.dot(f)
                        dist = max(dist, need)
    else:
        dist = radius / max(math.tan(math.radians(20)), 1e-3) * FIT_MARGIN

    cam = bpy.data.objects.new("cam", cam_data)
    cam.location = center + cam_dir * dist
    scene.collection.objects.link(cam)
    scene.camera = cam
    # Aim at centre.
    look = (center - cam.location).normalized()
    cam.rotation_euler = look.to_track_quat('-Z', 'Y').to_euler()

    # Key + fill + rim lights.
    def add_sun(name, energy, rot_deg):
        light = bpy.data.lights.new(name, 'SUN')
        light.energy = energy
        obj = bpy.data.objects.new(name, light)
        obj.rotation_euler = tuple(math.radians(d) for d in rot_deg)
        scene.collection.objects.link(obj)
    add_sun("key", 4.2 * LIGHT_SCALE, (52, 8, 40 + AZIMUTH_DEG))     # warm key
    add_sun("fill", 1.6 * LIGHT_SCALE, (66, -20, 200 + AZIMUTH_DEG))  # cool fill
    add_sun("rim", 2.2 * LIGHT_SCALE, (28, 0, 150 + AZIMUTH_DEG))     # rim pop

    os.makedirs(os.path.dirname(os.path.abspath(OUT)), exist_ok=True)
    scene.render.filepath = OUT
    bpy.ops.render.render(write_still=True)
    print(f"RENDERED {OUT}")


if __name__ == "__main__":
    main()
