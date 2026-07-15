#!/usr/bin/env python3
"""Build the consumables as two-primitive Blender glbs, in the same faceted cel
style as the weapons/explosives families.

One piece today, one glb at assets/items/<id>/model.glb:
  bandage : a rolled linen strip with a loose tail unrolling out of it.
            prim0 = the roll + its retaining band (bandage_roll),
            prim1 = the loose tail strip (bandage_tail).

WHY TWO PRIMS: the tail is animated independently of the roll. The engine's
`held_piece_local_transform` scales the tail out from the roll as the use charge
builds, so the bandage visibly UNROLLS in the player's hand. That per-piece
animation is only addressable if the tail is its own primitive, exactly like the
bow's limbs / string / arrow.

REFERENCE FRAME: authored Blender Z-up, exported +Y up via export_yup=True, so
authoring (x, y, z) -> in-game (x, z, -y).
  - The roll's cylinder AXIS runs along authoring X  -> in-game X (across view).
  - The tail unrolls along authoring +Y             -> in-game -Z (away from the
    player, i.e. forward), which is the direction it needs to grow in.
  - The tail's ROOT sits at the bottom tangent of the roll, authoring
    (0, 0, -ROLL_R) -> in-game (0, -ROLL_R, 0). That point is the pivot the Rust
    side scales the tail about (BANDAGE_TAIL_PIVOT in src/app/systems/items/held.rs);
    if you move it here, move it there.

PROPORTIONS are measured, not eyeballed: art/items/bandage/candidates/ref_2.png
(ComfyUI concept) measured with OpenCV gives roll width/diameter = 0.97 and
tail length/diameter = 1.36. Both are reproduced below.

COLOR_0 albedos are LINEAR (docs/rendering-materials.md), so these read lighter
in game than the numbers suggest. Both prims ride the engine's existing `Cloth`
material family, so no new material wiring is needed. Every mesh gets box UVs (a
toon material with no UVs renders INVISIBLE) and a COLOR_0 "Color" attribute set
as the render colour index.

Run headless:
  /Applications/Blender.app/Contents/MacOS/Blender -b -P art/consumables/build_consumables.py
Or one piece:
  ... -P art/consumables/build_consumables.py -- bandage [out.glb]

FULL PIPELINE: concept (ComfyUI) -> OpenCV measure -> this script -> glb ->
scripts/render_icon.py (mesh-rendered master) -> scripts/icon_finalize.py -> icon.
"""

import bpy
import bmesh
import math
import os
import sys

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ITEMS = os.path.join(REPO, "assets", "items")
TEXEL = 0.20   # metres per detail-texture tile (box projection)

# ---- linen palette (LINEAR albedo) ---------------------------------------------
# Clean, pale, slightly warm: this is bleached-ish linen, not the sackcloth brown
# the powder bomb / satchel use. It has to read as "medical" next to them.
#
# These sit well below where they "look" right as raw numbers: COLOR_0 is LINEAR,
# so 0.62 would land near 0.81 sRGB and blow out to a flat white that loses all
# form. Keep the mid around 0.45 and let the light do the lifting.
LINEN = (0.50, 0.43, 0.30)       # linen face
LINEN_LT = (0.64, 0.56, 0.41)    # lit coil crest / raised weave
LINEN_DK = (0.25, 0.20, 0.13)    # coil groove / shadowed fold
BAND = (0.42, 0.33, 0.21)        # the tan strip tied around the roll
BAND_DK = (0.26, 0.20, 0.12)

# ---- measured proportions (see docstring) ---------------------------------------
ROLL_R = 0.100                   # roll radius -> 0.20 m diameter
ROLL_HW = 0.097                  # half-width along the axis (w/dia = 0.97)
TAIL_LEN = 0.272                 # tail length (len/dia = 1.36)
TAIL_HW = 0.088                  # tail half-width (slightly inside the roll)
TAIL_T = 0.006                   # tail half-thickness: a flat strip
SIDES = 14                       # roll facet count: round enough at icon size


def lerp3(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t)


def box_uv(co, n):
    """Triplanar box UV: project on the plane facing the dominant normal axis so
    the detail texture tiles without polar pinch or hard stretch."""
    ax = max(range(3), key=lambda i: abs(n[i]))
    if ax == 0:
        u, w = co.y, co.z
    elif ax == 1:
        u, w = co.x, co.z
    else:
        u, w = co.x, co.y
    return (u / TEXEL, w / TEXEL)


def ring_x(x, r, sides, phase=0.0):
    """An n-gon ring in the YZ plane at position `x` along the roll axis."""
    return [
        (x, math.cos(phase + 2.0 * math.pi * i / sides) * r,
            math.sin(phase + 2.0 * math.pi * i / sides) * r)
        for i in range(sides)
    ]


class Builder:
    """Accumulates faceted pieces into one bmesh, tagging each with a material
    index (0 / 1) so the export splits into two primitives. Each piece is
    recalc'd on its own faces only, then joined. COLOR_0 alpha is 0.0 throughout:
    the ember GLOW MASK convention is inert here, nothing on a bandage emits."""

    def __init__(self):
        self.bm = bmesh.new()
        self.col = self.bm.loops.layers.float_color.new("Color")
        self.uv = self.bm.loops.layers.uv.new("UVMap")

    def _finish(self, faces, color_of, smooth):
        bmesh.ops.recalc_face_normals(self.bm, faces=faces)
        self.bm.normal_update()
        for f in faces:
            f.smooth = smooth
            for lp in f.loops:
                lp[self.col] = (*color_of(lp.vert), 0.0)
                lp[self.uv].uv = box_uv(lp.vert.co, f.normal)

    def add_stack(self, rings, color_of, mat_index, smooth=False,
                  cap_first=True, cap_last=True):
        """Bridge a list of equal-length rings into ONE closed piece with a single
        recalc (keeps recalc_face_normals reliable, see build_weapons.py)."""
        vrings = [[self.bm.verts.new(p) for p in ring] for ring in rings]
        self.bm.verts.ensure_lookup_table()
        faces = []
        for ra, rb in zip(vrings, vrings[1:]):
            n = len(ra)
            for i in range(n):
                j = (i + 1) % n
                faces.append(self.bm.faces.new([ra[i], ra[j], rb[j], rb[i]]))
        if cap_first:
            faces.append(self.bm.faces.new(list(reversed(vrings[0]))))
        if cap_last:
            faces.append(self.bm.faces.new(vrings[-1]))
        for f in faces:
            f.material_index = mat_index
        self._finish(faces, color_of, smooth)
        return faces

    def add_strip(self, spine, color_of, mat_index, smooth=False):
        """A flat ribbon swept along `spine` = [(x, y, z, half_width, half_thick,
        roll), ...]. Each cross-section is a thin rectangle spanning +/-half_width
        across and +/-half_thick through, banked by `roll` radians about the
        travel axis so the cloth can twist. Used for the unrolling tail."""
        rings = []
        for (x, y, z, hw, ht, roll) in spine:
            c, s = math.cos(roll), math.sin(roll)
            # Across-axis and through-axis, both banked by `roll`.
            ax, az = c * hw, s * hw
            tx, tz = -s * ht, c * ht
            rings.append([
                (x - ax - tx, y, z - az - tz),
                (x + ax - tx, y, z + az - tz),
                (x + ax + tx, y, z + az + tz),
                (x - ax + tx, y, z - az + tz),
            ])
        return self.add_stack(rings, color_of, mat_index, smooth)

    def to_object(self, name):
        me = bpy.data.meshes.new(name)
        self.bm.to_mesh(me)
        self.bm.free()
        if me.color_attributes:
            idx = me.color_attributes.find("Color")
            me.color_attributes.render_color_index = idx
            me.color_attributes.active_color_index = idx
        me.update()
        obj = bpy.data.objects.new(name, me)
        bpy.context.collection.objects.link(obj)
        return obj


# ============================================================ BANDAGE ============
def build_bandage(b):
    """A rolled linen strip with its loose tail unrolling forward.

    prim0 = the roll (body + a spiralled COIL dished into each end) + the tan
            retaining band.
    prim1 = the loose tail strip, rooted at the roll's bottom tangent.

    The coil is what makes this read as a ROLL and not a plain cylinder at 160px:
    concentric ledges stepped into each end face, tone-banded light on the crest
    and dark in the groove, so the spiral catches the eye even when the icon is
    tiny.

    NORMALS: the whole roll is built as ONE CLOSED staircase lathe about X, from
    the hub of the -X face, out to the rim, along the barrel, and back into the
    hub of the +X face. `recalc_face_normals` is only reliable on a piece that
    encloses a volume; an open dished cap (which is what the coil "obviously"
    wants to be) comes out inside-out and renders as a black hole under Bevy's
    backface culling. Every step below moves monotonically inward in x and
    monotonically down in radius, so the lathe stays a valid closed solid."""

    # --- roll (prim0): one closed staircase lathe. Profile is (x, radius) pairs;
    # a pair that moves only in x is a coil LEDGE wall, a pair that moves only in
    # radius is the flat annulus between ledges. ---
    def roll_color(v):
        r = math.hypot(v.co.y, v.co.z) / ROLL_R
        on_barrel = r > 0.97
        if on_barrel:
            # Soft top-to-bottom gradient so the barrel has form even flat-lit,
            # plus a faint circumferential weave ripple.
            t = (v.co.z / ROLL_R) * 0.5 + 0.5
            weave = 0.5 + 0.5 * math.cos(math.atan2(v.co.z, v.co.y) * SIDES)
            base = lerp3(LINEN_DK, LINEN, 0.35 + 0.65 * t)
            return lerp3(base, LINEN_LT, 0.14 * weave)
        # On an end face: band by radius so the coil reads as a spiral. Darken
        # hard toward the hub so the core reads as a hole you could poke.
        band = 0.5 + 0.5 * math.cos(r * math.pi * 5.0)
        face = lerp3(LINEN_DK, LINEN_LT, 0.30 + 0.70 * band)
        return lerp3(LINEN_DK, face, min(1.0, r * 2.6))

    # One face's ledges, rim -> hub, as (depth_into_roll, radius_fraction).
    ledges = [
        (0.000, 1.00),
        (0.005, 1.00),   # ledge wall
        (0.005, 0.80),   # annulus
        (0.009, 0.80),
        (0.009, 0.62),
        (0.013, 0.62),
        (0.013, 0.46),
        (0.017, 0.46),
        (0.017, 0.32),
        (0.021, 0.32),
        (0.021, 0.20),
        (0.030, 0.20),
        (0.030, 0.09),   # hub lip
    ]
    profile = []
    # -X face: hub first (deepest) out to the rim, so x increases throughout.
    for depth, rf in reversed(ledges):
        profile.append((-ROLL_HW + depth, ROLL_R * rf))
    # +X face: rim back in to the hub, x still increasing.
    for depth, rf in ledges:
        profile.append((ROLL_HW - depth, ROLL_R * rf))
    b.add_stack(
        [ring_x(x, r, SIDES) for (x, r) in profile],
        roll_color, 0, smooth=False, cap_first=True, cap_last=True,
    )

    # --- retaining band (prim0): the tan strip tied around the roll's middle,
    # standing slightly proud so it breaks up the cylinder silhouette. Closed
    # (capped) for the same normals reason as the roll; the caps land inside the
    # barrel and are never seen. ---
    def band_color(v):
        t = (v.co.z / ROLL_R) * 0.5 + 0.5
        return lerp3(BAND_DK, BAND, 0.4 + 0.6 * t)

    b.add_stack(
        [
            ring_x(-0.028, ROLL_R * 0.99, SIDES),
            ring_x(-0.021, ROLL_R * 1.05, SIDES),
            ring_x(0.021, ROLL_R * 1.05, SIDES),
            ring_x(0.028, ROLL_R * 0.99, SIDES),
        ],
        band_color, 0, smooth=False, cap_first=True, cap_last=True,
    )

    # --- tail (prim1): a linen strip rooted at the roll's BOTTOM tangent, peeling
    # off and running forward (+Y). Its root vertex sits exactly at
    # (0, 0, -ROLL_R), which is the pivot the engine scales this prim about as the
    # bandage unrolls, so do not move it without moving BANDAGE_TAIL_PIVOT.
    #
    # Cloth, not a plank: the strip droops under its own weight, banks (twists)
    # as it falls, wanders a little off-axis, and narrows toward the loose end.
    def tail_color(v):
        # Fade toward the loose end (worn, less lit) and ripple along the length
        # so the flat strip catches light unevenly like real cloth.
        along = (v.co.y / TAIL_LEN) if TAIL_LEN else 0.0
        ripple = 0.5 + 0.5 * math.cos(v.co.y * 46.0)
        base = lerp3(LINEN, LINEN_DK, 0.34 * along)
        return lerp3(base, LINEN_LT, 0.16 * ripple)

    spine = []
    steps = 12
    for i in range(steps + 1):
        t = i / steps
        y = TAIL_LEN * t
        # Peel off the roll's underside: hug it briefly, then fall away, then a
        # slight upward curl at the very tip the way a loose end lifts.
        droop = -0.052 * t * t
        curl = 0.026 * max(0.0, t - 0.70) / 0.30
        # A lazy sideways wander, and a bank that grows down the length, so no two
        # cross-sections are coplanar.
        drift = 0.020 * math.sin(t * 2.4)
        roll = 0.30 * t * t
        half_w = TAIL_HW * (1.0 - 0.16 * t)
        spine.append((drift, y, -ROLL_R + droop + curl, half_w, TAIL_T, roll))
    b.add_strip(spine, tail_color, 1, smooth=False)


# ============================================================ EXPORT =============
CONSUMABLES = {
    "bandage": ("bandage", build_bandage, ("bandage_roll", False),
                ("bandage_tail", False)),
}


def make_slot_material(name, is_metal):
    """A minimal placeholder material per slot. The engine replaces it at load; it
    exists only so export_materials='EXPORT' emits a distinct primitive per slot
    and wires COLOR_0 into Base Color."""
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nt = m.node_tree
    bsdf = nt.nodes.get("Principled BSDF")
    bsdf.inputs["Metallic"].default_value = 0.7 if is_metal else 0.0
    bsdf.inputs["Roughness"].default_value = 0.45 if is_metal else 0.9
    vc = nt.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    m.use_backface_culling = True
    return m


def build_one(key, out_path):
    item_id, fn, (slot0_name, slot0_metal), (slot1_name, slot1_metal) = CONSUMABLES[key]
    bpy.ops.wm.read_homefile(use_empty=True)
    b = Builder()
    fn(b)
    obj = b.to_object(item_id)
    # Two NAMED material slots so the exporter emits two primitives.
    obj.data.materials.append(make_slot_material(slot0_name, is_metal=slot0_metal))
    obj.data.materials.append(make_slot_material(slot1_name, is_metal=slot1_metal))
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    # export_vertex_color="NAME" (not "ACTIVE"): with export_materials="EXPORT",
    # the ACTIVE/MATERIAL modes strip COLOR_0 to VEC3 unless the material's alpha
    # input is driven by it. NAME mode dumps the named attribute verbatim as VEC4.
    # See art/explosives/build_explosives.py for the full write-up.
    bpy.ops.export_scene.gltf(
        filepath=out_path, export_format="GLB", use_selection=True,
        export_yup=True, export_apply=True, export_normals=True,
        export_materials="EXPORT", export_texcoords=True,
        export_vertex_color="NAME", export_vertex_color_name="Color",
    )
    lo = [min(v.co[i] for v in obj.data.vertices) for i in range(3)]
    hi = [max(v.co[i] for v in obj.data.vertices) for i in range(3)]
    print(f"EXPORTED {out_path}")
    print(f"  authoring bounds (Z-up): "
          f"x {lo[0]:.3f}..{hi[0]:.3f}  y {lo[1]:.3f}..{hi[1]:.3f}  "
          f"z {lo[2]:.3f}..{hi[2]:.3f}")
    print(f"  tris={sum(len(p.vertices) - 2 for p in obj.data.polygons)}")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if argv:
        key = argv[0]
        out = argv[1] if len(argv) > 1 else os.path.join(
            ITEMS, CONSUMABLES[key][0], "model.glb")
        build_one(key, out)
    else:
        for key, (item_id, *_rest) in CONSUMABLES.items():
            build_one(key, os.path.join(ITEMS, item_id, "model.glb"))


if __name__ == "__main__":
    main()
