#!/usr/bin/env python3
"""Build the armor pieces (P4a) as rig-attachment glbs: chunky low-poly
faceted cel shells that parent onto the third-person player rig joints with an
identity transform and sit ~10-15% proud of the underlying part boxes.

Three sets x four slots = 12 pieces, one glb per piece at
assets/items/<id>/model.glb:

  padded  (cloth):        padded_hood, padded_tunic, padded_leggings, padded_wraps
  lamellar(wood slats):   lamellar_helm, lamellar_vest, lamellar_greaves, lamellar_boots
  iron    (riveted plate): iron_helm, iron_cuirass, iron_greaves, iron_boots

PRIMITIVE CONVENTION (exactly, consistently):
  prim0 = <id>_shell : the main shell (hood/torso/thigh/boot).
  prim1 = <id>_aux   : the auxiliary attachment where one exists. Only the three
                       CHEST pieces have prim1 (a shoulder / pauldron cap). The
                       other nine pieces are single-prim (shell only).
The shoulder/pauldron cap is authored pivot-local to UpperArmR's joint frame (the
RIGHT shoulder); the engine mirrors it for the left. So a chest glb carries geometry
in TWO pivot spaces: prim0 in the Body pivot, prim1 in the UpperArmR pivot. The
systems package attaches prim0 to the Body part and prim1 (mirrored) to each upper
arm.

REFERENCE FRAME (matches the weapons pipeline and the rig ground truth
src/app/scene/mesh/player.rs). We author in Blender Z-up and export export_yup=True.
Measured axis remap (raw glTF): Blender +X -> glTF +X, Blender +Z -> glTF +Y (up),
Blender +Y -> glTF -Z. The rig lives in Bevy Y-up, -Z forward. Therefore in AUTHORING
Blender space:
  Blender +Z  == in-game +Y  (up)
  Blender +Y  == in-game -Z  (FORWARD; the rig faces -Z, so visors/seams face +Y here)
  Blender +X  == in-game +X  (the rig's right side)
Each shell is authored PIVOT-LOCAL: origin at the joint, so identity attach fits.
A rig part box at in-game (x, y, z) with half (hx, hy, hz) is, in authoring Blender
space, centred at (x, z, y) with half (hx, hz, hy). We simply pass Blender coords
directly (add_box takes Blender center/half); the mapping is applied by hand in each
builder and documented per piece.

RIG GROUND TRUTH (in-game coords, from player.rs), the boxes each shell wraps:
  Body chest   c(0, 0.30, 0)   half(0.20, 0.16, 0.125)
  Body waist   c(0, 0.06, 0)   half(0.15, 0.10, 0.11)
  Body pelvis  c(0,-0.13, 0)   half(0.155,0.075,0.105)
  Body head    c(0, 0.64, 0)   half(0.105,0.11, 0.10)   crown top ~ y 0.795
  Body neck    c(0, 0.49, 0)   half(0.05, 0.045,0.05)
  shoulder joint (0.20, 0.46, 0); UpperArm boxes z=-0.025 half(0.058,0.045,0.062),
                                                  z=-0.16  half(0.05, 0.11, 0.052)
  ForearmR boxes down to z=-0.30 (hand); ForearmR joint at upper-arm-local z=-0.26.
  hip joint (0.105,-0.14,0); Thigh box y=-0.18 half(0.072,0.18,0.082), len 0.36.
  knee joint (thigh-local y=-0.36); Shin box y=-0.16 half(0.062,0.16,0.078),
    boot cuff y=-0.30 half(0.082,0.04,0.09), boot foot c(0,-0.355,-0.03) half(0.072,0.045,0.11).

Every mesh is a closed manifold, recalc_face_normals per piece, box/triplanar UVs
(a toon material with no UVs renders invisible), and a COLOR_0 "Color" attribute set
as the render colour index. COLOR_0 carries SET IDENTITY only; the engine detail
textures (cloth weave / wood slats / steel) carry the grain. Albedos are LINEAR.

Run headless (all twelve):
  /Applications/Blender.app/Contents/MacOS/Blender -b -P art/armor/build_armor.py
Or one piece:
  ... -P art/armor/build_armor.py -- <id> [out.glb]
"""

import bpy
import bmesh
import math
import os
import sys

from mathutils import Matrix, Vector

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ITEMS = os.path.join(REPO, "assets", "items")

TEXEL = 0.20   # metres per detail-texture tile (box projection); armour panels are
#                larger than weapon parts so a slightly coarser tile keeps the weave
#                / slats / rivets readable rather than micro-tiled.

# ---- COLOR_0 palette (LINEAR albedos, see docs/rendering-materials.md) ----------
# padded  = warm undyed cloth tones (quilted canvas / linen, leather straps).
# lamellar = wood-slat browns over a dark cloth backing.
# iron     = neutral steel grey with darker rivet dots (metal COLOR_0 drives F0, so
#            the greys are bright on purpose; rivets are a darker punctuation).

# padded (cloth)
CLOTH = (0.42, 0.35, 0.24)         # warm undyed quilted canvas
CLOTH_DK = (0.30, 0.24, 0.16)      # quilt-seam shadow / hood interior
CLOTH_LT = (0.52, 0.44, 0.31)      # raised quilt puff
STRAP = (0.20, 0.13, 0.07)         # dark leather belt / buckle strap
STRAP_LT = (0.30, 0.20, 0.11)

# lamellar (wood slats over dark cloth)
SLAT = (0.34, 0.21, 0.10)          # oak slat (a touch darker/redder than tool oak)
SLAT_DK = (0.22, 0.13, 0.06)       # slat gap / shadowed row
SLAT_LT = (0.44, 0.29, 0.15)       # lit slat crest
BACKING = (0.24, 0.22, 0.17)       # dark cloth backing showing between rows
LAM_STRAP = (0.18, 0.12, 0.07)     # lacing / strap

# iron (riveted plate)
STEEL = (0.60, 0.62, 0.66)         # plate steel (bright; drives F0 on metal slot)
STEEL_DK = (0.40, 0.42, 0.46)      # bevel / recessed shadow
STEEL_LT = (0.78, 0.80, 0.85)      # edge / crest highlight
RIVET = (0.24, 0.25, 0.27)         # darker rivet dot / eye-slit void
STEEL_TRIM = (0.34, 0.24, 0.14)    # small warm leather trim (belt) to break the grey


def lerp3(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t)


def hash01(a, b):
    """Deterministic pseudo-random in [0,1) so the build is reproducible."""
    return (math.sin(a * 12.9898 + b * 78.233) * 43758.5453) % 1.0


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


class Builder:
    """Accumulates faceted pieces into one bmesh, tagging each with a material
    index (0 = shell, 1 = aux cap) so the export splits into up to two primitives.
    Each piece is recalc'd on its own faces only, then joined (a global recalc
    mis-guesses at interpenetrations). Copied from art/weapons/build_weapons.py
    with the armour-specific additions (rounded_box / slat_row / dome)."""

    def __init__(self):
        self.bm = bmesh.new()
        self.col = self.bm.loops.layers.float_color.new("Color")
        self.uv = self.bm.loops.layers.uv.new("UVMap")
        self.slots_used = set()

    def _finish_piece(self, faces, color_of, smooth):
        bmesh.ops.recalc_face_normals(self.bm, faces=faces)
        self.bm.normal_update()
        for f in faces:
            f.smooth = smooth
            for lp in f.loops:
                lp[self.col] = (*color_of(lp.vert), 1.0)
                lp[self.uv].uv = box_uv(lp.vert.co, f.normal)

    def _tag(self, faces, mat_index):
        self.slots_used.add(mat_index)
        for f in faces:
            f.material_index = mat_index

    def add_box(self, center, half, color_of, mat_index, smooth=False, rot=None):
        """Axis-aligned (optionally rotated) box, 6 quad faces."""
        cx, cy, cz = center
        hx, hy, hz = half
        signs = [(-1, -1, -1), (1, -1, -1), (1, 1, -1), (-1, 1, -1),
                 (-1, -1, 1), (1, -1, 1), (1, 1, 1), (-1, 1, 1)]
        vs = []
        for sx, sy, sz in signs:
            local = Vector((sx * hx, sy * hy, sz * hz))
            if rot is not None:
                local = rot @ local
            vs.append(self.bm.verts.new((cx + local.x, cy + local.y, cz + local.z)))
        self.bm.verts.ensure_lookup_table()
        quads = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
                 (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
        faces = [self.bm.faces.new([vs[k] for k in q]) for q in quads]
        self._tag(faces, mat_index)
        self._finish_piece(faces, color_of, smooth)
        return faces

    def add_prism(self, ring_a, ring_b, color_of, mat_index, smooth=False,
                  cap_a=True, cap_b=True):
        """Bridge two equal-length vertex rings into a tube/frustum."""
        va = [self.bm.verts.new(p) for p in ring_a]
        vb = [self.bm.verts.new(p) for p in ring_b]
        self.bm.verts.ensure_lookup_table()
        faces = []
        n = len(va)
        for i in range(n):
            j = (i + 1) % n
            faces.append(self.bm.faces.new([va[i], va[j], vb[j], vb[i]]))
        if cap_a and n >= 3:
            faces.append(self.bm.faces.new(list(reversed(va))))
        if cap_b and n >= 3:
            faces.append(self.bm.faces.new(vb))
        self._tag(faces, mat_index)
        self._finish_piece(faces, color_of, smooth)
        return faces

    def add_stack(self, rings, color_of, mat_index, smooth=False,
                  cap_first=True, cap_last=True):
        """Bridge a whole list of equal-length rings into ONE closed piece with a
        single recalc (keeps recalc_face_normals reliable vs stacked open bands)."""
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
        self._tag(faces, mat_index)
        self._finish_piece(faces, color_of, smooth)
        return faces

    def add_dome(self, center, radius, color_of, mat_index, oval=(1.0, 1.0, 1.0),
                 smooth=True, cut_below=None):
        """A faceted icosphere (subdiv 1) scaled per-axis for helmet crowns and
        shoulder caps. `cut_below` (a z value) deletes verts below it so a dome can
        sit as a half-shell on a segment; the exposed rim is left open (the shell it
        caps closes the manifold visually, and box UV keeps it textured)."""
        res = bmesh.ops.create_icosphere(self.bm, subdivisions=1, radius=radius)
        verts = res["verts"]
        cx, cy, cz = center
        ox, oy, oz = oval
        keep = []
        for v in verts:
            v.co.x = v.co.x * ox + cx
            v.co.y = v.co.y * oy + cy
            v.co.z = v.co.z * oz + cz
            keep.append(v)
        faces = set()
        for v in keep:
            for f in v.link_faces:
                faces.add(f)
        faces = list(faces)
        if cut_below is not None:
            drop = [f for f in faces if all(vv.co.z < cut_below for vv in f.verts)]
            if drop:
                bmesh.ops.delete(self.bm, geom=drop, context="FACES")
                faces = [f for f in faces if f.is_valid]
        self._tag(faces, mat_index)
        self._finish_piece(faces, color_of, smooth=smooth)
        return faces

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


def ngon_ring(cx, cy, z, rx, ry, sides, phase=0.0):
    """A regular n-gon ring centred at (cx,cy,z), radii (rx,ry)."""
    ring = []
    for i in range(sides):
        a = phase + 2.0 * math.pi * i / sides
        ring.append((cx + math.cos(a) * rx, cy + math.sin(a) * ry, z))
    return ring


def box_ring(cx, cy, z, hx, hy, phase=0.0):
    """A 4-vertex rounded-rectangle ring (for torso/limb shells that read boxy but
    not razor-cornered). Corners are bevelled by pulling them in 18%."""
    k = 0.82
    return [
        (cx + hx, cy + hy * k, z), (cx + hx * k, cy + hy, z),
        (cx - hx * k, cy + hy, z), (cx - hx, cy + hy * k, z),
        (cx - hx, cy - hy * k, z), (cx - hx * k, cy - hy, z),
        (cx + hx * k, cy - hy, z), (cx + hx, cy - hy * k, z),
    ]


# =====================================================================
# Shared clearance helper: shells sit CLEAR of the underlying part box by
# a scale factor on the box half-extents (1.10-1.15 -> 10-15% larger). On the
# THIN limb segments a pure +absolute pad blows the ratio past 15%, so limbs use
# the scale factor alone (PAD_LIMB small) and only the torso/head, which are big
# enough to swallow it, add a little absolute room.
# =====================================================================
CLEAR = 1.12       # 12% proud of the underlying rig box (mid of the 10-15% band)
PAD_LIMB = 0.006   # tiny absolute clearance for thin limb shells (keeps ratio ~1.15)
PAD_TORSO = 0.010  # a little absolute room on the torso/head shells


# =====================================================================
# PADDED SET (cloth) : quilted, soft, rounded. Warm undyed tones.
# =====================================================================
def _quilt_seams(b, stations, mat, n_rows=3, n_cols=4, depth=0.006):
    """Lay a shallow diamond-quilt grid of recessed seam grooves over a shell whose
    outline is given by `stations` = [(z, hx, hy, y_off), ...] top->bottom. A few
    thin horizontal seam bands (slightly inset, darker) plus short vertical dashes
    read as quilting at icon size without a texture. Cloth only."""
    ztop, zbot = stations[0][0], stations[-1][0]

    def interp(z):
        for a, c in zip(stations, stations[1:]):
            if a[0] >= z >= c[0]:
                t = (a[0] - z) / max(a[0] - c[0], 1e-6)
                return (a[1] + (c[1] - a[1]) * t, a[2] + (c[2] - a[2]) * t,
                        a[3] + (c[3] - a[3]) * t)
        return stations[-1][1:]
    # horizontal seam bands (inset, darker)
    for r in range(1, n_rows + 1):
        z = ztop - (ztop - zbot) * r / (n_rows + 1)
        hx, hy, yo = interp(z)
        band = [box_ring(0, yo, z + depth, hx * 0.99, hy * 0.99),
                box_ring(0, yo, z - depth, hx * 0.99, hy * 0.99)]
        b.add_stack(band, lambda v: CLOTH_DK, mat, smooth=False,
                    cap_first=False, cap_last=False)
def _face_opening(b, head_z, hx, hy, dark):
    """Frame a face opening on the FRONT (the rig faces -Z, which for the head maps
    to Blender +y). A recessed dark face panel set into the front plane plus a ring
    of raised beads around it, so the head reads as a helmet/cowl with a face, not a
    featureless bag. (A literal through-hole would show a culled hollow interior at
    icon and in-game, so we recess a dark panel instead.)"""
    # recessed dark oval face panel, set just inside the front plane (+y). One clean
    # sunken shape reads as "a face is here" far better than a ring of little beads.
    panel = [
        box_ring(0, hy * 0.82, head_z - 0.085, hx * 0.34, hy * 0.10),  # chin (narrow)
        box_ring(0, hy * 0.80, head_z - 0.02, hx * 0.58, hy * 0.15),   # cheeks (wide)
        box_ring(0, hy * 0.80, head_z + 0.05, hx * 0.56, hy * 0.14),   # eyes
        box_ring(0, hy * 0.84, head_z + 0.11, hx * 0.42, hy * 0.11),   # brow (narrow)
    ]
    b.add_stack(panel, lambda v: dark, 0, smooth=True, cap_first=True, cap_last=True)


def build_padded_hood(b):
    """Soft quilted cowl over the Body head region (pivot at Body/root origin; z up,
    the rig's forward -Z maps to Blender +y here so the face opening looks +y). A
    rounded cloth hood wrapping the back/sides of the head with a pointed peak and
    an open face, draping onto the shoulders. Single prim (shell). Head box in-game
    c(0,0.64) half(.105,.11,.10); crown ~0.795."""
    HEAD_Z = 0.64
    hx, hy = 0.105 * CLEAR + PAD_TORSO, 0.10 * CLEAR + PAD_TORSO   # ~0.128 / 0.122

    def hood_color(v):
        # back (-y) lit, quilt banding, gathered peak a touch darker
        band = 0.5 + 0.5 * math.sin(v.co.z * 46.0)
        base = CLOTH_LT if band > 0.55 else CLOTH
        return lerp3(base, CLOTH_DK, 0.25) if v.co.z > HEAD_Z + 0.12 else base

    # Cowl shell: rounded rings shoulder->crown, the crown gathered to a peak that
    # leans back (-y). The front is left OPEN (we cap first only) and rimmed below.
    rings = [
        box_ring(0, 0.0, HEAD_Z - 0.16, hx * 1.05, hy * 1.15),   # shoulder drape
        box_ring(0, 0.0, HEAD_Z - 0.06, hx * 1.05, hy * 1.08),   # jaw
        box_ring(0, -0.005, HEAD_Z + 0.03, hx * 1.06, hy * 1.06),  # cheeks (widest)
        box_ring(0, -0.020, HEAD_Z + 0.12, hx * 0.88, hy * 0.92),  # temples, lean back
        box_ring(0, -0.045, HEAD_Z + 0.19, hx * 0.42, hy * 0.5),   # crown gather peak
    ]
    b.add_stack(rings, hood_color, 0, smooth=True, cap_first=True, cap_last=True)
    # a small quilted peak tip leaning back
    b.add_dome((0, -0.06, HEAD_Z + 0.21), 0.06, hood_color, 0,
               oval=(0.7, 0.9, 0.8), smooth=True)
    _face_opening(b, HEAD_Z, hx, hy, CLOTH_DK)


def build_padded_tunic(b):
    """Quilted torso shell (prim0, Body pivot) + one shoulder cap (prim1, UpperArmR
    pivot; engine mirrors for L). Chest box in-game c(0,0.30) half(.20,.16,.125),
    waist c(0,0.06) half(.15,.10,.11), pelvis c(0,-0.13) half(.155,.075,.105)."""
    # ---- prim0: quilted torso shell, root-local (Blender z up) ----
    def cloth_color(v):
        # subtle vertical quilt banding via z; puffs read lighter
        band = 0.5 + 0.5 * math.sin(v.co.z * 34.0)
        base = CLOTH_LT if band > 0.55 else CLOTH
        # front (toward -... the chest, +y is forward here) a touch lit
        return base
    hx_c, hy_c = 0.20 * CLEAR, 0.125 * CLEAR      # ~0.224 / 0.14
    hx_w, hy_w = 0.15 * CLEAR, 0.11 * CLEAR
    hx_p, hy_p = 0.155 * CLEAR, 0.105 * CLEAR
    rings = [
        box_ring(0, 0, 0.44, hx_c * 0.7, hy_c * 0.8),    # collar (above chest)
        box_ring(0, 0, 0.30, hx_c, hy_c),                # chest (widest)
        box_ring(0, 0, 0.16, hx_c * 0.86, hy_c * 0.92),  # lower chest
        box_ring(0, 0, 0.06, hx_w, hy_w),                # waist (taper)
        box_ring(0, 0, -0.06, hx_p * 0.98, hy_p),        # belt line
        box_ring(0, 0, -0.17, hx_p, hy_p * 1.02),        # skirt hem (padded tunic drops past pelvis)
    ]
    b.add_stack(rings, cloth_color, 0, smooth=False, cap_first=True, cap_last=True)
    # quilt seam grid over the chest so the cloth reads padded, not slab
    _quilt_seams(b, [(0.42, hx_c * 0.95, hy_c, 0.0), (0.30, hx_c, hy_c, 0.0),
                     (0.16, hx_c * 0.86, hy_c * 0.92, 0.0), (0.06, hx_w, hy_w, 0.0)],
                 0, n_rows=3)
    # Belt strap across the waist (dark leather) + buckle.
    for z in (-0.05,):
        belt = [box_ring(0, 0, z + 0.018, hx_p * 1.02, hy_p * 1.03),
                box_ring(0, 0, z - 0.018, hx_p * 1.02, hy_p * 1.03)]
        b.add_stack(belt, lambda v: STRAP, 0, smooth=False, cap_first=False, cap_last=False)
    b.add_box((0, -hy_p * 1.02, -0.05), (0.032, 0.010, 0.026), lambda v: STRAP_LT, 0)  # buckle
    # Fringed hem accent (a slightly darker band at the very bottom).
    hemband = [box_ring(0, 0, -0.165, hx_p, hy_p * 1.02),
               box_ring(0, 0, -0.185, hx_p * 0.97, hy_p)]
    b.add_stack(hemband, lambda v: CLOTH_DK, 0, smooth=False, cap_first=False, cap_last=True)

    _padded_shoulder_cap(b)


def _padded_shoulder_cap(b):
    """prim1: a soft quilted shoulder cap authored pivot-local to UpperArmR's joint
    (in-game shoulder at (0.20,0.46,0); pivot-local the cap sits just below/around
    the joint at Blender z ~ 0..-0.06). A rounded pad draping over the top of the
    upper arm. Engine mirrors this for the left shoulder."""
    def cap_color(v):
        return lerp3(CLOTH, CLOTH_LT, 0.5)
    # Upper-arm shoulder box in-game z=-0.025 half(.058,.045,.062) -> Blender.
    hx, hy = 0.058 * CLEAR + 0.02, 0.062 * CLEAR + 0.02
    rings = [
        box_ring(0, 0, 0.02, hx * 0.9, hy * 0.9),
        box_ring(0, 0, -0.03, hx, hy),
        box_ring(0, 0, -0.09, hx * 0.95, hy * 0.98),
    ]
    b.add_stack(rings, cap_color, 1, smooth=True, cap_first=True, cap_last=True)
    # A layered second flap for the padded look (a wider soft pauldron edge).
    b.add_dome((0, 0, -0.01), hx * 1.15, cap_color, 1,
               oval=(1.0, 1.05, 0.55), smooth=True, cut_below=-0.06)


def build_padded_leggings(b):
    """One quilted thigh shell (prim0), engine mirrors L/R. Pivot at the hip joint;
    Blender z down = in-game down. Thigh box in-game y=-0.18 half(.072,.18,.082)."""
    def cloth_color(v):
        band = 0.5 + 0.5 * math.sin(v.co.z * 30.0)
        return CLOTH_LT if band > 0.55 else CLOTH
    hx, hy = 0.072 * CLEAR + PAD_LIMB, 0.082 * CLEAR + PAD_LIMB   # ~0.093 / 0.104
    rings = [
        box_ring(0, 0, -0.01, hx * 1.02, hy * 1.05),   # hip top (flared)
        box_ring(0, 0, -0.12, hx, hy),                 # mid thigh
        box_ring(0, 0, -0.26, hx * 0.9, hy * 0.92),    # above knee
        box_ring(0, 0, -0.35, hx * 0.82, hy * 0.86),   # knee (leggings stop here)
    ]
    b.add_stack(rings, cloth_color, 0, smooth=False, cap_first=True, cap_last=True)
    # quilt seam bands down the thigh
    _quilt_seams(b, [(-0.01, hx * 1.02, hy * 1.05, 0.0), (-0.12, hx, hy, 0.0),
                     (-0.26, hx * 0.9, hy * 0.92, 0.0), (-0.35, hx * 0.82, hy * 0.86, 0.0)],
                 0, n_rows=3)
    # A quilted knee patch (raised puff, on the FRONT +y).
    b.add_dome((0, hy * 0.7, -0.33), 0.045, lambda v: CLOTH_LT, 0,
               oval=(1.1, 0.6, 0.9), smooth=True)


def build_padded_wraps(b):
    """One shin/boot cloth wrap shell (prim0), mirrored. Pivot at the knee joint.
    Shin box in-game y=-0.16 half(.062,.16,.078); boot cuff y=-0.30 half(.082,.04,.09);
    boot foot c(0,-0.355,-0.03) half(.072,.045,.11) (toe forward = -z = Blender +y)."""
    def wrap_color(v):
        band = 0.5 + 0.5 * math.sin(v.co.z * 40.0)     # crossed-wrap banding
        return CLOTH_LT if band > 0.5 else CLOTH
    hx, hy = 0.062 * CLEAR + PAD_LIMB, 0.078 * CLEAR + PAD_LIMB
    rings = [
        box_ring(0, 0, -0.02, hx * 0.95, hy * 0.95),   # below knee
        box_ring(0, 0, -0.16, hx, hy),                 # mid shin
        box_ring(0, 0, -0.30, hx * 1.05, hy * 1.05),   # ankle wrap (flared)
    ]
    b.add_stack(rings, wrap_color, 0, smooth=False, cap_first=True, cap_last=False)
    # Cloth foot wrap over the boot foot: the toe walks FORWARD (+y) and down, so the
    # foot centre shifts toward +y as z descends (matches the rig boot foot which is
    # forward of the ankle).
    foot = [box_ring(0, 0.0, -0.31, hx * 1.02, hy),
            box_ring(0, 0.03, -0.355, 0.082 * CLEAR, 0.10 * CLEAR),
            box_ring(0, 0.07, -0.40, 0.06 * CLEAR, 0.11 * CLEAR)]
    b.add_stack(foot, wrap_color, 0, smooth=False, cap_first=False, cap_last=True)
    # crossed lacing bands (dark) up the shin
    for z in (-0.08, -0.16, -0.24):
        band = [box_ring(0, 0, z + 0.012, hx * 1.03, hy * 1.03),
                box_ring(0, 0, z - 0.012, hx * 1.03, hy * 1.03)]
        b.add_stack(band, lambda v: STRAP, 0, smooth=False, cap_first=False, cap_last=False)


# =====================================================================
# LAMELLAR SET (wood slats over cloth): horizontal slat rows, chunky.
# =====================================================================
def _slat_rows(b, rings_top_bot, n_rows, mat, color_top, color_bot, backing_color):
    """Given a list of (z, hx, hy, y_off) stations top->bottom, lay n_rows of
    horizontal slat bands, each row a short vertical box_ring sandwich standing
    slightly proud of a darker backing shell. Returns nothing (adds faces)."""
    zs = [s[0] for s in rings_top_bot]
    ztop, zbot = zs[0], zs[-1]

    def interp(z):
        for a, c in zip(rings_top_bot, rings_top_bot[1:]):
            if a[0] >= z >= c[0]:
                t = (a[0] - z) / max(a[0] - c[0], 1e-6)
                return (a[1] + (c[1] - a[1]) * t, a[2] + (c[2] - a[2]) * t,
                        a[3] + (c[3] - a[3]) * t)
        return rings_top_bot[-1][1:]
    # backing shell (dark cloth) slightly inset
    brings = [box_ring(0, s[3], s[0], s[1] * 0.97, s[2] * 0.97) for s in rings_top_bot]
    b.add_stack(brings, lambda v: backing_color, mat, smooth=False,
                cap_first=True, cap_last=True)
    # slat rows proud of the backing
    for r in range(n_rows):
        zc = ztop - (r + 0.5) * (ztop - zbot) / n_rows
        zt = zc + 0.42 * (ztop - zbot) / n_rows
        zb = zc - 0.42 * (ztop - zbot) / n_rows
        hxt, hyt, yt = interp(zt)
        hxb, hyb, yb = interp(zb)
        t_shade = r / max(n_rows - 1, 1)
        col = lerp3(color_top, color_bot, t_shade)

        def rowcol(v, col=col):
            crest = 0.5 + 0.5 * math.sin(v.co.z * 120.0)
            return lerp3(col, SLAT_LT, 0.25 * crest)
        row = [box_ring(0, yt, zt, hxt * 1.04, hyt * 1.04),
               box_ring(0, (yt + yb) / 2, zc, hxt * 1.06, hyt * 1.06),
               box_ring(0, yb, zb, hxb * 1.04, hyb * 1.04)]
        b.add_stack(row, rowcol, mat, smooth=False, cap_first=False, cap_last=False)


def build_lamellar_helm(b):
    """Pointed slat hood-helm over the Body head region (concept: a peaked cap of
    horizontal wood slats with a cloth aventail curtain over the nape/cheeks and an
    open face). Reads distinctly TOUGHER than the padded hood (hard slats, a sharper
    apex) and DISTINCT from the iron helm (no smooth dome, no visor). Single prim.
    Head centre Blender z=0.64. Forward (face) = Blender +y."""
    HEAD_Z = 0.64
    hx, hy = 0.105 * CLEAR + PAD_TORSO, 0.10 * CLEAR + PAD_TORSO
    # cloth aventail: a dark backing curtain wrapping nape + cheeks (open face +y)
    curtain = [
        box_ring(0, -0.01, HEAD_Z - 0.14, hx * 1.06, hy * 1.12),   # neck curtain
        box_ring(0, -0.01, HEAD_Z - 0.02, hx * 1.02, hy * 1.04),   # cheeks
        box_ring(0, -0.01, HEAD_Z + 0.06, hx * 0.98, hy * 0.98),   # temples
    ]
    b.add_stack(curtain, lambda v: BACKING, 0, smooth=True, cap_first=True, cap_last=False)
    # pointed slat cap: horizontal slat rows narrowing to a sharp apex, leaning back
    rows_stations = [
        (HEAD_Z + 0.20, hx * 0.30, hy * 0.34, -0.05),  # apex
        (HEAD_Z + 0.13, hx * 0.66, hy * 0.70, -0.03),
        (HEAD_Z + 0.05, hx * 0.98, hy * 1.00, -0.01),  # widest
        (HEAD_Z - 0.03, hx * 1.02, hy * 1.02, 0.0),    # brow band
    ]
    _slat_rows(b, rows_stations, 5, 0, SLAT_LT, SLAT, BACKING)
    # a hard slat brow-peak jutting forward over the face opening (+y)
    b.add_box((0, hy * 0.9, HEAD_Z + 0.0), (hx * 0.66, 0.045, 0.024),
              lambda v: SLAT, 0, smooth=False,
              rot=Matrix.Rotation(math.radians(20), 4, Vector((1, 0, 0))))
    # sharp apex spike
    b.add_dome((0, -0.05, HEAD_Z + 0.22), 0.05, lambda v: SLAT_DK, 0,
               oval=(0.6, 0.7, 1.1), smooth=False)
    _face_opening(b, HEAD_Z, hx, hy, SLAT_DK)


def build_lamellar_vest(b):
    """Horizontal slat-row torso (prim0) + slatted shoulder cap (prim1, UpperArmR).
    Same torso stations as padded but rendered as wood slat rows over dark cloth."""
    stations = [
        (0.42, 0.20 * CLEAR * 0.72, 0.125 * CLEAR * 0.8, 0.0),   # collar
        (0.30, 0.20 * CLEAR, 0.125 * CLEAR, 0.0),                # chest
        (0.16, 0.20 * CLEAR * 0.86, 0.125 * CLEAR * 0.92, 0.0),  # lower chest
        (0.06, 0.15 * CLEAR, 0.11 * CLEAR, 0.0),                 # waist
        (-0.08, 0.155 * CLEAR, 0.105 * CLEAR, 0.0),              # belt line
    ]
    _slat_rows(b, stations, 6, 0, SLAT_LT, SLAT, BACKING)
    # dark cloth skirt below the slat rows (lamellar vests skirt in cloth, per concept)
    hx_p, hy_p = 0.155 * CLEAR, 0.105 * CLEAR
    skirt = [box_ring(0, 0, -0.08, hx_p, hy_p),
             box_ring(0, 0, -0.20, hx_p * 1.06, hy_p * 1.02)]
    b.add_stack(skirt, lambda v: BACKING, 0, smooth=False, cap_first=False, cap_last=True)
    # leather belt across the waist
    belt = [box_ring(0, 0, -0.06, hx_p * 1.03, hy_p * 1.03),
            box_ring(0, 0, -0.02, hx_p * 1.03, hy_p * 1.03)]
    b.add_stack(belt, lambda v: LAM_STRAP, 0, smooth=False, cap_first=False, cap_last=False)

    _lamellar_shoulder_cap(b)


def _lamellar_shoulder_cap(b):
    """prim1: slatted shoulder cap over UpperArmR (mirrored L). Two short slat rows
    arcing over the top of the upper arm on a dark backing."""
    hx, hy = 0.058 * CLEAR + 0.02, 0.062 * CLEAR + 0.02
    b.add_dome((0, 0, -0.02), hx * 1.2, lambda v: BACKING, 1,
               oval=(1.05, 1.1, 0.6), smooth=True, cut_below=-0.08)
    stations = [
        (0.03, hx * 0.9, hy * 0.95, 0.0),
        (-0.03, hx * 1.05, hy * 1.05, 0.0),
        (-0.08, hx * 1.0, hy * 1.0, 0.0),
    ]
    _slat_rows(b, stations, 2, 1, SLAT_LT, SLAT, BACKING)


def build_lamellar_greaves(b):
    """Slatted thigh shell (prim0), mirrored. Slat rows down the thigh over backing.
    Thigh box in-game y=-0.18 half(.072,.18,.082)."""
    hx, hy = 0.072 * CLEAR + PAD_LIMB, 0.082 * CLEAR + PAD_LIMB
    stations = [
        (-0.01, hx * 1.02, hy * 1.05, 0.0),
        (-0.12, hx, hy, 0.0),
        (-0.26, hx * 0.9, hy * 0.92, 0.0),
        (-0.35, hx * 0.82, hy * 0.86, 0.0),
    ]
    _slat_rows(b, stations, 5, 0, SLAT_LT, SLAT, BACKING)
    # knee cop (a chunkier slat plate over the knee, on the FRONT +y)
    b.add_box((0, hy * 0.9, -0.33), (hx * 0.7, 0.03, 0.05),
              lambda v: SLAT_DK, 0, smooth=False)


def build_lamellar_boots(b):
    """Slatted shin/boot shell (prim0), mirrored. Shin slat rows + a wood-capped
    boot foot (toe forward = Blender +y)."""
    hx, hy = 0.062 * CLEAR + PAD_LIMB, 0.078 * CLEAR + PAD_LIMB
    stations = [
        (-0.02, hx * 0.95, hy * 0.95, 0.0),
        (-0.16, hx, hy, 0.0),
        (-0.29, hx * 1.05, hy * 1.05, 0.0),
    ]
    _slat_rows(b, stations, 4, 0, SLAT_LT, SLAT, BACKING)
    # wood boot foot over the rig boot: toe walks FORWARD (+y) and down
    foot = [box_ring(0, 0.0, -0.30, hx * 1.02, hy),
            box_ring(0, 0.03, -0.355, 0.082 * CLEAR, 0.10 * CLEAR),
            box_ring(0, 0.07, -0.40, 0.072 * CLEAR, 0.11 * CLEAR)]
    b.add_stack(foot, lambda v: SLAT, 0, smooth=False, cap_first=False, cap_last=True)
    # toe cap darker (forward +y)
    b.add_box((0, 0.11, -0.40), (0.05, 0.04, 0.03), lambda v: SLAT_DK, 0, smooth=False)


# =====================================================================
# IRON SET (riveted plate): smooth steel shells, rivet dots, eye slit.
# =====================================================================
def _rivets(b, points, r, mat):
    """Scatter small dark rivet domes at the given (x,y,z) points."""
    for (x, y, z) in points:
        b.add_dome((x, y, z), r, lambda v: RIVET, mat,
                   oval=(1.0, 0.7, 1.0), smooth=True)


def build_iron_helm(b):
    """Riveted plate bascinet: a smooth rounded steel skull, a raised crown COMB, a
    clear brow band, and a dark rectangular VISOR with a breathing grille on the
    front (+y). The visor + comb are the signature that must read at icon size and
    separate this from the padded cowl and the slat helm. Single prim. Head centre
    Blender z=0.64. Forward (face) = Blender +y."""
    HEAD_Z = 0.64
    hx, hy = 0.105 * CLEAR + PAD_TORSO, 0.10 * CLEAR + PAD_TORSO   # ~0.128 / 0.122

    def steel_color(v):
        crest = 0.5 + 0.5 * math.sin(v.co.z * 26.0)
        return lerp3(STEEL, STEEL_LT, 0.25 * crest)

    # rounded skull: a stack of rings (not a bare icosphere) so it reads as a helmet
    # bowl with a defined brow, curving over and back.
    skull = [
        box_ring(0, -0.005, HEAD_Z - 0.10, hx * 1.02, hy * 1.02),   # nape / jaw
        box_ring(0, 0.0, HEAD_Z - 0.01, hx * 1.05, hy * 1.05),      # brow line (widest)
        box_ring(0, -0.005, HEAD_Z + 0.08, hx * 0.94, hy * 0.96),   # temples
        box_ring(0, -0.015, HEAD_Z + 0.15, hx * 0.62, hy * 0.66),   # crown curve
        box_ring(0, -0.02, HEAD_Z + 0.20, hx * 0.24, hy * 0.28),    # top
    ]
    b.add_stack(skull, steel_color, 0, smooth=True, cap_first=True, cap_last=True)
    # brow band (a bright raised ring of steel just above the visor)
    brow = [box_ring(0, 0.0, HEAD_Z + 0.01, hx * 1.06, hy * 1.06),
            box_ring(0, 0.0, HEAD_Z - 0.03, hx * 1.06, hy * 1.06)]
    b.add_stack(brow, lambda v: STEEL_LT, 0, smooth=False, cap_first=False, cap_last=False)
    # comb: a continuous raised crest running front-back (Blender y) over the crown,
    # built as one swept ridge so it reads as a single fin, not tabs.
    comb_path = [
        (0, hy * 0.85, HEAD_Z + 0.055), (0, hy * 0.45, HEAD_Z + 0.15),
        (0, 0.0, HEAD_Z + 0.205), (0, -hy * 0.5, HEAD_Z + 0.185),
        (0, -hy * 0.85, HEAD_Z + 0.10),
    ]
    comb_rings = []
    for (x, y, z) in comb_path:
        comb_rings.append([(x + 0.013, y, z + 0.0), (x + 0.013, y - 0.03, z),
                           (x - 0.013, y - 0.03, z), (x - 0.013, y, z)])
    # sweep the fin as a thin vertical blade: use box-section stack along the path
    for a, c in zip(comb_path, comb_path[1:]):
        mid = ((a[0] + c[0]) / 2, (a[1] + c[1]) / 2, (a[2] + c[2]) / 2)
        dy = abs(c[1] - a[1]) / 2 + 0.006
        dz = abs(c[2] - a[2]) / 2 + 0.020
        b.add_box(mid, (0.013, dy, dz), lambda v: STEEL_LT, 0, smooth=False)
    # VISOR: a forward dark plate over the face (+y), pivoted slightly so its lower
    # edge juts out (a proper mask), with a bright frame ring so it separates.
    b.add_box((0, hy * 1.02, HEAD_Z - 0.035), (hx * 0.68, 0.032, 0.082),
              lambda v: STEEL_DK, 0, smooth=False,
              rot=Matrix.Rotation(math.radians(8), 4, Vector((1, 0, 0))))
    # bright visor frame (top brow edge of the mask)
    b.add_box((0, hy * 1.05, HEAD_Z + 0.048), (hx * 0.7, 0.02, 0.012),
              lambda v: STEEL_LT, 0, smooth=False)
    # eye slit: a wide dark void across the visor at eye height
    b.add_box((0, hy * 1.10, HEAD_Z + 0.015), (hx * 0.6, 0.02, 0.014),
              lambda v: RIVET, 0, smooth=False)
    # grille: rows of breathing holes on the lower visor (clearly read as a mask)
    for row_z, xs in ((HEAD_Z - 0.04, (-0.052, -0.018, 0.018, 0.052)),
                      (HEAD_Z - 0.075, (-0.036, 0.0, 0.036)),
                      (HEAD_Z - 0.105, (-0.02, 0.02))):
        _rivets(b, [(x, hy * 1.12, row_z) for x in xs], 0.011, 0)
    # rivets on the brow band
    _rivets(b, [(math.sin(a) * hx * 0.95, math.cos(a) * hy * 0.4 + hy * 0.55,
                 HEAD_Z + 0.0) for a in (-0.9, -0.35, 0.35, 0.9)], 0.012, 0)


def build_iron_cuirass(b):
    """Plate torso (prim0) + pauldron cap (prim1, UpperArmR; mirrored). A smooth
    steel breastplate with a central ridge, a fauld (skirt of plate) and rivets,
    plus a warm leather belt. Chest box in-game half(.20,.16,.125)."""
    def steel_color(v):
        # centre ridge (near x=0, front +y) reads brighter
        edge = min(1.0, abs(v.co.x) / (0.20 * CLEAR))
        return lerp3(STEEL_LT, STEEL, 0.3 + 0.6 * edge)
    hx_c, hy_c = 0.20 * CLEAR, 0.125 * CLEAR
    hx_w, hy_w = 0.15 * CLEAR, 0.11 * CLEAR
    hx_p, hy_p = 0.155 * CLEAR, 0.105 * CLEAR
    # front (+y) bulges forward: the breastplate crowns toward the camera.
    rings = [
        box_ring(0, 0, 0.45, hx_c * 0.6, hy_c * 0.72),    # gorget / collar
        box_ring(0, 0.012, 0.34, hx_c * 0.95, hy_c),      # upper chest (forward bulge)
        box_ring(0, 0.02, 0.24, hx_c, hy_c * 1.04),       # chest (widest breastplate)
        box_ring(0, 0.01, 0.12, hx_c * 0.84, hy_c * 0.92),  # lower breastplate
        box_ring(0, 0, 0.04, hx_w, hy_w),                 # waist
        box_ring(0, 0, -0.05, hx_p * 0.98, hy_p),         # belt line
    ]
    b.add_stack(rings, steel_color, 0, smooth=True, cap_first=True, cap_last=True)
    # raised central KEEL ridge down the FRONT (+y) of the breastplate: ONE bold
    # vertical crest (twin thin tabs read as floating rectangles at icon size),
    # proud of the plate by ~0.025-0.033 so it shows in silhouette and shading,
    # flanked by a darker recessed crease line each side so the ridge pops even
    # under flat light. Ring cy/hy chosen so inner edges stay buried in the
    # curved plate along the whole run (front face y ~ 0.124-0.166 over z).
    keel = [box_ring(0, hy_c * 0.98, 0.40, 0.020, 0.020),
            box_ring(0, hy_c * 1.10, 0.30, 0.030, 0.030),   # proudest at the chest
            box_ring(0, hy_c * 1.08, 0.18, 0.028, 0.026),
            box_ring(0, hy_c * 0.96, 0.06, 0.020, 0.018)]
    b.add_stack(keel, lambda v: STEEL_LT, 0, smooth=False, cap_first=True, cap_last=True)
    for sx in (-1, 1):
        crease = [box_ring(sx * 0.062, hy_c * 1.16, 0.30, 0.014, 0.014),
                  box_ring(sx * 0.066, hy_c * 1.16, 0.23, 0.015, 0.015),
                  box_ring(sx * 0.062, hy_c * 1.16, 0.16, 0.014, 0.014)]
        b.add_stack(crease, lambda v: STEEL_DK, 0, smooth=False,
                    cap_first=True, cap_last=True)
    # fauld: a skirt of overlapping plate below the belt
    fauld = [box_ring(0, 0, -0.05, hx_p, hy_p),
             box_ring(0, 0, -0.15, hx_p * 1.08, hy_p * 1.02),
             box_ring(0, 0, -0.19, hx_p * 1.02, hy_p * 0.98)]
    b.add_stack(fauld, lambda v: STEEL_DK, 0, smooth=False, cap_first=False, cap_last=True)
    # warm leather belt to break the grey
    belt = [box_ring(0, 0, -0.03, hx_p * 1.02, hy_p * 1.02),
            box_ring(0, 0, 0.01, hx_p * 1.02, hy_p * 1.02)]
    b.add_stack(belt, lambda v: STEEL_TRIM, 0, smooth=False, cap_first=False, cap_last=False)
    b.add_box((0, hy_p * 1.03, -0.01), (0.03, 0.012, 0.024), lambda v: STEEL_LT, 0)  # buckle (front)
    # rivets on the FRONT (+y) of the breastplate: sized and placed to read at the
    # front-3/4 icon angle (bigger than the first pass, pulled inboard off the
    # silhouette edge, centred just inside the plate surface so ~40% pokes out).
    _rivets(b, [(sx * hx_c * 0.62, hy_c * 1.12, 0.24 + dz)
                for sx in (-1, 1) for dz in (0.09, 0.0, -0.09)], 0.020, 0)

    _iron_pauldron_cap(b)


def _iron_pauldron_cap(b):
    """prim1: a domed steel pauldron over UpperArmR (mirrored L). A layered plate
    shoulder with a rivet ring."""
    hx, hy = 0.058 * CLEAR + 0.02, 0.062 * CLEAR + 0.02
    b.add_dome((0, 0, -0.01), hx * 1.3, lambda v: STEEL, 1,
               oval=(1.1, 1.2, 0.7), smooth=True, cut_below=-0.07)
    # a second overlapping lower lame (plate strip)
    lame = [box_ring(0, 0, -0.05, hx * 1.25, hy * 1.2),
            box_ring(0, 0, -0.10, hx * 1.1, hy * 1.05)]
    b.add_stack(lame, lambda v: STEEL_DK, 1, smooth=False, cap_first=False, cap_last=True)
    _rivets(b, [(math.cos(a) * hx * 1.0, math.sin(a) * hy * 0.6 - 0.02, 0.02)
                for a in (0.6, math.pi / 2, math.pi - 0.6)], 0.012, 1)


def build_iron_greaves(b):
    """Plate thigh shell (prim0), mirrored. Smooth steel cuisse + knee cop + rivets.
    Thigh box in-game half(.072,.18,.082)."""
    def steel_color(v):
        edge = min(1.0, abs(v.co.x) / (0.072 * CLEAR + PAD_LIMB))
        return lerp3(STEEL_LT, STEEL, 0.3 + 0.6 * edge)
    hx, hy = 0.072 * CLEAR + PAD_LIMB, 0.082 * CLEAR + PAD_LIMB
    rings = [
        box_ring(0, 0, -0.01, hx * 1.02, hy * 1.05),
        box_ring(0, -0.01, -0.12, hx, hy),
        box_ring(0, 0, -0.26, hx * 0.9, hy * 0.92),
        box_ring(0, 0, -0.36, hx * 0.85, hy * 0.9),
    ]
    b.add_stack(rings, steel_color, 0, smooth=True, cap_first=True, cap_last=True)
    # knee cop: a domed plate over the knee, on the FRONT (+y)
    b.add_dome((0, hy * 0.85, -0.34), 0.055, lambda v: STEEL_LT, 0,
               oval=(1.1, 0.7, 1.0), smooth=True)
    _rivets(b, [(sx * hx * 0.75, hy * 0.9, -0.12) for sx in (-1, 1)], 0.012, 0)


def build_iron_boots(b):
    """Plate shin/boot shell (prim0), mirrored. Steel greave-shin + sabaton foot.
    Shin box half(.062,.16,.078); boot foot toe forward (Blender +y)."""
    def steel_color(v):
        edge = min(1.0, abs(v.co.x) / (0.062 * CLEAR + PAD_LIMB))
        return lerp3(STEEL_LT, STEEL, 0.3 + 0.6 * edge)
    hx, hy = 0.062 * CLEAR + PAD_LIMB, 0.078 * CLEAR + PAD_LIMB
    rings = [
        box_ring(0, 0, -0.02, hx * 0.95, hy * 0.95),
        box_ring(0, -0.01, -0.16, hx, hy),
        box_ring(0, 0, -0.29, hx * 1.05, hy * 1.05),
    ]
    b.add_stack(rings, steel_color, 0, smooth=True, cap_first=True, cap_last=False)
    # sabaton foot: the toe points FORWARD (+y). Rig boot foot in-game is
    # c(0,-0.355,-0.03) with -z (forward) toe = Blender +y, so the foot walks toward
    # +y and down. Length runs +y as it descends.
    foot = [box_ring(0, 0.0, -0.30, hx * 1.02, hy),
            box_ring(0, 0.03, -0.355, 0.082 * CLEAR, 0.09 * CLEAR),
            box_ring(0, 0.07, -0.40, 0.06 * CLEAR, 0.11 * CLEAR)]
    b.add_stack(foot, steel_color, 0, smooth=True, cap_first=False, cap_last=True)
    # pointed toe cap (forward +y)
    b.add_box((0, 0.12, -0.40), (0.045, 0.05, 0.028), lambda v: STEEL_DK, 0, smooth=False,
              rot=Matrix.Rotation(math.radians(-10), 4, Vector((1, 0, 0))))
    _rivets(b, [(sx * hx * 0.75, hy * 0.9, -0.16) for sx in (-1, 1)], 0.011, 0)


# ============================================================ EXPORT ==============
# id -> (builder, is_metal_shell, has_aux)
PIECES = {
    "padded_hood": (build_padded_hood, False, False),
    "padded_tunic": (build_padded_tunic, False, True),
    "padded_leggings": (build_padded_leggings, False, False),
    "padded_wraps": (build_padded_wraps, False, False),
    "lamellar_helm": (build_lamellar_helm, False, False),
    "lamellar_vest": (build_lamellar_vest, False, True),
    "lamellar_greaves": (build_lamellar_greaves, False, False),
    "lamellar_boots": (build_lamellar_boots, False, False),
    "iron_helm": (build_iron_helm, True, False),
    "iron_cuirass": (build_iron_cuirass, True, True),
    "iron_greaves": (build_iron_greaves, True, False),
    "iron_boots": (build_iron_boots, True, False),
}


def make_slot_material(name, is_metal):
    """A minimal placeholder material per slot. The engine replaces it at load; it
    exists only so export_materials='EXPORT' emits a distinct primitive per slot and
    wires COLOR_0 into Base Color."""
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nt = m.node_tree
    bsdf = nt.nodes.get("Principled BSDF")
    bsdf.inputs["Metallic"].default_value = 0.7 if is_metal else 0.0
    bsdf.inputs["Roughness"].default_value = 0.45 if is_metal else 0.85
    vc = nt.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    m.use_backface_culling = True
    return m


def build_one(item_id, out_path):
    fn, is_metal, has_aux = PIECES[item_id]
    bpy.ops.wm.read_homefile(use_empty=True)
    b = Builder()
    fn(b)
    obj = b.to_object(item_id)
    # Slot 0 = shell (always), slot 1 = aux cap (only where has_aux). Both slots
    # get the set's metalness so the engine's placeholder swap is consistent.
    shell_mat = make_slot_material(item_id + "_shell", is_metal=is_metal)
    obj.data.materials.append(shell_mat)
    if has_aux:
        aux_mat = make_slot_material(item_id + "_aux", is_metal=is_metal)
        obj.data.materials.append(aux_mat)
        assert 1 in b.slots_used, f"{item_id} declares aux but no faces tagged slot 1"
    else:
        assert b.slots_used == {0}, \
            f"{item_id} is single-prim but tagged slots {b.slots_used}"
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=out_path, export_format="GLB", use_selection=True,
        export_yup=True, export_apply=True, export_normals=True,
        export_materials="EXPORT", export_texcoords=True,
        export_vertex_color="ACTIVE",
    )
    print(f"EXPORTED {out_path}")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if argv:
        item_id = argv[0]
        out = argv[1] if len(argv) > 1 else os.path.join(ITEMS, item_id, "model.glb")
        build_one(item_id, out)
    else:
        for item_id in PIECES:
            build_one(item_id, os.path.join(ITEMS, item_id, "model.glb"))


if __name__ == "__main__":
    main()
