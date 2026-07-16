#!/usr/bin/env python3
"""Build the iron sickle held-item glb, matched to the shipped ComfyUI icon.

The sickle is the grass-harvesting tool (one sweep reaps a Tall Grass tuft,
see the hay node in src/resource_nodes.rs). Iron-only by design.

ICON-FIRST: the shipped icon is the rembg cutout of the chosen ComfyUI
generation (art/concepts/sickle/icon3_v6.png, a photoreal classic grain
sickle: thin blackened forged crescent, bright ground edge on the concave
side, tall steel collar, curved rosewood handle with a butt knob). This
mesh is built to MATCH that icon, and the match is MEASURED, not eyeballed:
art/tools/measure_sickle.py segments the cutout and emits the blade
centreline stations + ribbon half-widths in handle-lengths (the collar-top
and butt-tip anchor points are read off the image by hand because the
blade's rust patches defeat pure hue segmentation). The station table below
is pasted from that run.

Authoring conventions are inherited wholesale from art/weapons/build_weapons.py
(this script imports its Builder + palette + helpers): authored Z-up, exported
+Y up; grip along Blender +Z with the pommel at Z ~= -0.514 so the hand pose
lines up with the other tools; the blade bows toward -X with its sharp edge on
the concave (+X-facing) side, so in hand the hook arcs up and inward with the
point hanging down (the owner-approved carry); the thin axis is Blender +Y.
Two material slots so the exporter emits two primitives:

  prim 0 = sickle_grip (wood family: the curved rosewood handle)
  prim 1 = sickle_head (iron family, metal slot: collar + forged blade)

COLOR_0 albedos are LINEAR. The blade deliberately does NOT reuse the bright
sword palette: forged near-black flats + a narrow bright ground-edge band +
sparse rust mottle are what make thin steel read as metal under the cel
shader (the previous pale flat-grey ribbon read as clay, owner report).

The blade is swept as a knife cross-section, not a box: a sharp single-vertex
edge on the concave side, flat cheeks, and a blunt rounded spine. Half
thickness stays ~6 mm-equivalent so the blade is actually thin.

Run headless (model only, the normal case):
  /Applications/Blender.app/Contents/MacOS/Blender -b -P art/tools/build_sickle.py
Model + a preview render for eyeball-matching against the icon (NOT shipped):
  ... -P art/tools/build_sickle.py -- preview /tmp/sickle_preview.png
"""

import math
import os
import sys

import bpy

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, "art", "weapons"))

import build_weapons as W  # Builder, palettes, helpers

ITEM_ID = "iron_sickle"
OUT_GLB = os.path.join(ROOT, "assets", "items", ITEM_ID, "model.glb")

HAFT_BOT = -0.514   # shared tool pommel height (grips line up in hand)
HAFT_TOP = 0.10     # collar seats here
S = HAFT_TOP - HAFT_BOT  # authored units per icon handle-length (0.614)
SIDES = 10

# Forged-steel palette for the blade (linear). Deliberately MUCH darker than
# the shared sword IRON family: the icon's blade is blackened wrought iron,
# and the in-hand viewmodel cel path multiplies albedo by a noon scene probe
# of roughly 3-4x (toon_viewmodel.wgsl `scene`), so authored values near
# 0.05-0.09 are what display as dark steel in hand (0.15 already showed as
# pale silver, measured off a headless noon screenshot).
FORGED = (0.050, 0.050, 0.065)    # flat cheeks
FORGED_LT = (0.090, 0.090, 0.110)  # worn sheen patches on the cheeks
SPINE = (0.085, 0.090, 0.100)     # blunt hammered back, catches the band light
EDGE_BRIGHT = (0.75, 0.78, 0.85)  # narrow ground cutting edge, gleams to white
RUST = (0.090, 0.055, 0.035)      # sparse warm mottle


def build_sickle(b):
    """Curved rosewood handle (slot 0) + steel collar and forged blade (slot 1)."""

    # ---- handle: a gentle J in the XZ plane (icon: the grip bows slightly
    # away from the blade through the belly and curls back toward it at the
    # bulbous butt knob). Rings stay parallel XY n-gons, so the stack remains
    # one closed manifold; offsets are small enough that the in-hand grip
    # pose still reads as gripping a straight haft.
    def haft_color(v):
        t = (v.co.z - HAFT_BOT) / (HAFT_TOP - HAFT_BOT)
        base = W.lerp3(W.WOOD_DK, W.WOOD, 0.40 + 0.30 * t)
        streak = W.hash01(round(math.atan2(v.co.y, v.co.x) * 3.0, 1), round(v.co.z * 9.0, 1))
        if streak > 0.72:
            return W.lerp3(base, W.WOOD_DK, 0.55)
        if streak < 0.12:
            return W.lerp3(base, W.WOOD_LT, 0.45)
        return base

    # (z, radius, x_offset): butt knob curls toward the blade (-x), the belly
    # bows away (+x), the top straightens into the collar.
    haft_rings = [
        (HAFT_BOT, 0.018, -0.012),
        (HAFT_BOT + 0.014, 0.034, -0.016),
        (HAFT_BOT + 0.036, 0.041, -0.010),   # bulbous butt knob
        (HAFT_BOT + 0.066, 0.038, 0.000),
        (-0.400, 0.0345, 0.007),
        (-0.330, 0.0335, 0.013),
        (-0.250, 0.0330, 0.016),             # belly of the bow
        (-0.160, 0.0330, 0.014),
        (-0.070, 0.0325, 0.010),
        (0.010, 0.0300, 0.005),
        (0.060, 0.0275, 0.002),
        (HAFT_TOP, 0.0250, 0.000),
        (HAFT_TOP + 0.030, 0.0230, 0.000),   # buried under the collar
    ]
    b.add_stack(
        [W.ngon_ring(xo, 0.0, z, r, r, SIDES) for (z, r, xo) in haft_rings],
        haft_color, 0, smooth=True,
    )

    # ---- steel collar (icon: a tall plain band between wood and blade) ----
    def collar_color(v):
        t = (v.co.z - HAFT_TOP) / 0.05
        return W.lerp3(W.lerp3(W.IRON_GUARD, W.IRON_LT, 0.25), W.IRON_GUARD, 0.4 * t)

    collar_profile = [
        (HAFT_TOP - 0.005, 0.0270),
        (HAFT_TOP, 0.0315),
        (HAFT_TOP + 0.032, 0.0300),
        (HAFT_TOP + 0.045, 0.0235),
    ]
    b.add_lathe(collar_profile, SIDES, collar_color, 1, smooth=True)

    # ---- blade: the measured icon crescent, swept as a thin knife section --
    # Centreline stations from measure_sickle.py on the icon cutout, in
    # handle-lengths relative to the collar top (x toward the tip side,
    # z up the haft). x is NEGATED here so the blade bows toward -X per the
    # authoring convention above. Root smoothed, tip extended to a point.
    ref = [
        (0.000, -0.010),
        (-0.004, 0.110),
        (-0.015, 0.225),   # near-straight riser off the collar
        (0.045, 0.321),    # the turn onto the crescent
        (0.161, 0.324),
        (0.255, 0.316),
        (0.335, 0.302),
        (0.411, 0.280),
        (0.487, 0.248),
        (0.566, 0.204),
        (0.654, 0.138),
        (0.734, 0.038),
        (0.800, -0.060),   # point hangs below the collar top
    ]
    # Ribbon half-widths per station (handle-lengths): near-constant slim
    # belly, a slightly broader turn, a fine point.
    hws = [0.055, 0.055, 0.058, 0.060, 0.055, 0.050, 0.047,
           0.046, 0.047, 0.048, 0.046, 0.036, 0.004]
    # Half-thicknesses per station (authored units): a boxy socket root
    # thinning to genuinely thin forged steel.
    hvs = [0.011, 0.009, 0.0078, 0.0070, 0.0066, 0.0064, 0.0062,
           0.0060, 0.0058, 0.0056, 0.0052, 0.0045, 0.0025]

    z0 = HAFT_TOP + 0.020  # blade root buried inside the collar
    # The measured crescent's circle-fit centre (flipped like the path):
    # the sharp edge faces this centre (the concave inside of the hook).
    centre = (-0.297 * S, z0 + 0.020 * S)

    def catmull(p0, p1, p2, p3, t):
        t2, t3 = t * t, t * t * t
        return tuple(
            0.5 * (2.0 * p1[i] + (-p0[i] + p2[i]) * t
                   + (2.0 * p0[i] - 5.0 * p1[i] + 4.0 * p2[i] - p3[i]) * t2
                   + (-p0[i] + 3.0 * p1[i] - 3.0 * p2[i] + p3[i]) * t3)
            for i in range(2)
        )

    def interp(t, vals):
        f = t * (len(vals) - 1)
        i = min(int(f), len(vals) - 2)
        return vals[i] + (vals[i + 1] - vals[i]) * (f - i)

    stations = 26
    pts, widths, thicks = [], [], []
    for k in range(stations + 1):
        t = k / stations
        f = t * (len(ref) - 1)
        i = min(int(f), len(ref) - 2)
        p0 = ref[max(i - 1, 0)]
        p3 = ref[min(i + 2, len(ref) - 1)]
        x, z = catmull(p0, ref[i], ref[i + 1], p3, f - i)
        pts.append((-x * S, z0 + z * S))
        widths.append(max(interp(t, hws) * S, 0.0025))
        thicks.append(interp(t, hvs))

    # The blade is authored in the XZ plane, then YAWED about the haft so its
    # flat face angles toward the first-person camera: a dead fore-aft blade
    # plane reads as a paper-thin line in hand and the sickle identity
    # vanishes (headless screenshot check on the first crescent).
    yaw = math.radians(-55.0)
    cy, sy = math.cos(yaw), math.sin(yaw)

    def place(x2, y_thick, z2):
        return (x2 * cy - y_thick * sy, x2 * sy + y_thick * cy, z2)

    rings = []       # list of vert-coord rings
    ring_cols = []   # matching per-vertex colors
    n = len(pts)
    for i in range(n):
        px, pz = pts[i]
        ax, az = pts[max(i - 1, 0)]
        bx, bz = pts[min(i + 1, n - 1)]
        tx, tz = bx - ax, bz - az
        tl = math.hypot(tx, tz) or 1.0
        tx, tz = tx / tl, tz / tl
        # In-plane normal pointing at the arc centre = the sharp-edge side.
        cx_, cz_ = centre[0] - px, centre[1] - pz
        dot = cx_ * tx + cz_ * tz
        nx, nz = cx_ - dot * tx, cz_ - dot * tz
        nl = math.hypot(nx, nz) or 1.0
        nx, nz = nx / nl, nz / nl

        hw, hv = widths[i], thicks[i]
        t = i / (n - 1)
        # Knife section: v0 = sharp edge, v1/v8 = the narrow ground bevel
        # (the ONLY bright band; per-corner colors interpolate across each
        # quad, so pinning the brightness to the outer 20% of the ribbon is
        # what keeps the edge line thin instead of silvering the whole
        # blade), v2/v7 + v3/v6 = near-black flat cheeks, v4/v5 = the blunt
        # hammered spine.
        us = [1.0, 0.80, 0.30, -0.50, -1.0, -1.0, -0.50, 0.30, 0.80]
        ws = [0.0, 0.45, 0.85, 1.00, 0.42, -0.42, -1.00, -0.85, -0.45]
        ring = [place(px + nx * hw * u, hv * w, pz + nz * hw * u)
                for u, w in zip(us, ws)]
        rings.append(ring)

        mott = W.hash01(px * 53.0, pz * 71.0)
        if t < 0.08:
            cols = [(0.060, 0.060, 0.075)] * 9   # plain dark socket tang
        else:
            cheek = FORGED
            if mott > 0.78:
                cheek = W.lerp3(FORGED, RUST, 0.55)
            elif mott < 0.12:
                cheek = W.lerp3(FORGED, FORGED_LT, 0.65)
            bevel = W.lerp3(EDGE_BRIGHT, FORGED_LT, 0.35)
            spine = W.lerp3(SPINE, RUST, 0.35) if mott > 0.90 else SPINE
            cols = [EDGE_BRIGHT, bevel, cheek, cheek, spine,
                    spine, cheek, cheek, bevel]
        ring_cols.append(cols)

    # Custom stack so each vertex keeps its own authored color (add_stack's
    # color_of(vert) callback cannot tell edge from spine verts).
    colmap = {}
    vrings = []
    for ring, cols in zip(rings, ring_cols):
        vs = [b.bm.verts.new(p) for p in ring]
        for v, c in zip(vs, cols):
            colmap[v] = c
        vrings.append(vs)
    b.bm.verts.ensure_lookup_table()
    faces = []
    for ra, rb in zip(vrings, vrings[1:]):
        m = len(ra)
        for i in range(m):
            j = (i + 1) % m
            faces.append(b.bm.faces.new([ra[i], ra[j], rb[j], rb[i]]))
    faces.append(b.bm.faces.new(list(reversed(vrings[0]))))
    faces.append(b.bm.faces.new(vrings[-1]))
    b._tag(faces, 1)
    b._finish_piece(faces, lambda v: colmap[v], smooth=True)


def export_model():
    bpy.ops.wm.read_homefile(use_empty=True)
    b = W.Builder()
    build_sickle(b)
    obj = b.to_object(ITEM_ID)
    for name, is_metal in [("grip", False), ("head", True)]:
        obj.data.materials.append(
            W.make_slot_material(f"{ITEM_ID}_{name}", is_metal=is_metal)
        )
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    os.makedirs(os.path.dirname(OUT_GLB), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=OUT_GLB, export_format="GLB", use_selection=True,
        export_yup=True, export_apply=True, export_normals=True,
        export_materials="EXPORT", export_texcoords=True,
        export_vertex_color="ACTIVE",
    )
    print(f"EXPORTED {OUT_GLB}")
    return obj


def render_preview(obj, out_path):
    """Render the freshly-built object to a PNG posed like the icon (handle
    leaning right, blade sweeping up-left, point hanging down) so the mesh
    can be eyeball-checked against the ComfyUI icon master. NOT the shipped
    icon (that is the icon3_v6 cutout itself)."""
    scene = bpy.context.scene

    # Un-yaw the blade about the haft (Z, applied first under ZYX order) so
    # the full crescent silhouette faces the camera, then lean the tool in
    # the image plane (about Y; the camera looks down -Y) to match the
    # icon's ~25 deg handle tilt.
    obj.rotation_mode = "ZYX"
    obj.rotation_euler = (0.0, math.radians(-25.0), math.radians(55.0))
    obj.location = (0.23, 0.0, 0.10)

    cam_data = bpy.data.cameras.new("icon_cam")
    cam_data.type = "ORTHO"
    cam_data.ortho_scale = 1.35
    cam = bpy.data.objects.new("icon_cam", cam_data)
    cam.location = (0.0, -2.0, 0.0)
    cam.rotation_euler = (math.radians(90.0), 0.0, 0.0)
    scene.collection.objects.link(cam)
    scene.camera = cam

    key = bpy.data.objects.new("icon_key", bpy.data.lights.new("icon_key", "SUN"))
    key.data.energy = 4.0
    key.rotation_euler = (math.radians(55.0), math.radians(-12.0), math.radians(-25.0))
    scene.collection.objects.link(key)
    fill = bpy.data.objects.new("icon_fill", bpy.data.lights.new("icon_fill", "SUN"))
    fill.data.energy = 1.2
    fill.rotation_euler = (math.radians(120.0), math.radians(15.0), math.radians(150.0))
    scene.collection.objects.link(fill)

    scene.render.engine = "CYCLES"
    scene.cycles.samples = 48
    scene.cycles.device = "CPU"
    scene.render.film_transparent = True
    scene.render.resolution_x = 512
    scene.render.resolution_y = 512
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGBA"
    scene.render.filepath = out_path
    bpy.ops.render.render(write_still=True)
    print(f"RENDERED {out_path}")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    obj = export_model()
    if argv and argv[0] == "preview":
        out = argv[1] if len(argv) > 1 else "/tmp/sickle_preview.png"
        render_preview(obj, out)


if __name__ == "__main__":
    main()
