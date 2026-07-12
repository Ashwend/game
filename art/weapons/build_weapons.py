#!/usr/bin/env python3
"""Build the melee AND ranged weapons as two-primitive Blender glbs:
chunky cel/anime silhouettes with a heavy, committed feel, low-poly and faceted.

Melee (P3a/b): wooden_club, stone_spear, iron_sword, iron_mace.
Ranged (P3c):  wooden_bow, crossbow, arrow.

One glb per weapon (assets/items/<id>/model.glb). Every weapon is authored as a
single mesh object carrying TWO material slots, which the gltf exporter splits
into TWO primitives:

  prim 0 = grip / haft   (wood family, or wrapped-wood grip on the sword)
  prim 1 = head / blade  (club head, stone point, iron blade+guard+pommel, mace head)

The arrow keeps the two-slot split:
  arrow:      prim 0 = wood shaft + fletching, prim 1 = knapped stone broadhead

The bow and crossbow instead carry MANY slots, one per animatable piece, so the
Rust viewmodel can play a draw (bow limbs flex + string pulls into a V) and a
cock/release (crossbow string slides forward/back). Each movable piece is its own
material slot / primitive with its geometry authored so the animator rotates or
translates it about a documented pivot coordinate (BOW_SLOTS / BOW_RIG and
CROSSBOW_SLOTS / CROSSBOW_RIG below). Strings ride a new Cord slot (*_string) so
they no longer inherit the wood material: the bow string is a pale waxed-linen
tint, the crossbow string a dark tarred cord.

The engine reuses this split: held_item_layers overlays one layer per primitive
and attaches a material family per layer (wood detail on prim 0, stone or iron on
prim 1). So the glb carries geometry, UVs, and COLOR_0 vertex colours only; the
detail textures are applied in Rust. We therefore export with
export_materials='EXPORT' and a DISTINCT material slot per primitive: 'NONE'
collapses the two-primitive split into one mesh and breaks per-layer material
assignment (hard-won gotcha, docs/playbooks/art-pipeline.md).

Reference frame (matches assets/items/iron_hatchet/model.glb so grips line up in
hand): authored Z-up, exported +Y up via export_yup=True. After export the haft
pommel sits at Y_min ~= -0.514 and the head top at Y_max ~= +0.356 (~0.87 tall).
In the AUTHORING frame that is Blender Z: pommel at Z ~= -0.514, head top at
Z ~= +0.356. Handle runs along Blender +Z (in-game up), the working edge / point
faces Blender +X (in-game forward, into the swing), the thin axis is Blender +Y.
The spear is a reach weapon and is allowed to run taller (head up to Z ~= +0.65),
but its GRIP section stays in the same place so the hand pose is unchanged.

COLOR_0 albedos are LINEAR (docs/rendering-materials.md): a mid tone eyeballed
perceptually renders ~1.5-2x too bright. Wood parts use the same warm oak the
deployables paint (WOOD = 0.46,0.30,0.16); stone parts use the shared ore rock
grey biased warm; iron parts use bright greys (metal COLOR_0 part-drives F0, not
diffuse, so bright is correct and must not be darkened toward dielectric values).

recalc_face_normals runs per solid piece (its own closed manifold) so winding
faces outward; a global recalc mis-guesses at interpenetrations. Every mesh gets
box/triplanar UVs (a toon material with no UVs renders invisible) and a COLOR_0
"Color" attribute set as the render colour index.

Run headless (all seven):
  /Applications/Blender.app/Contents/MacOS/Blender -b -P art/weapons/build_weapons.py
Or one weapon:
  ... -P art/weapons/build_weapons.py -- <club|spear|sword|mace|bow|crossbow|arrow> [out.glb]

FULL PIPELINE: concept (ComfyUI, offline this pass) -> this script -> glb ->
scripts/render_icon.py (mesh-rendered master) -> scripts/icon_finalize.py -> icon.
"""

import bpy
import bmesh
import math
import os
import sys

from mathutils import Matrix, Vector

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ITEMS = os.path.join(REPO, "assets", "items")

TEXEL = 0.16   # metres per detail-texture tile (box projection); weapons are small

# ---- COLOR_0 palette (LINEAR albedos, see docs/rendering-materials.md) ---------
# Wood matches the deployable oak so grips read as the same hardwood family.
# Stone matches the shared ore rock grey (warm-biased so the blue-sky IBL does not
# wash it pale). Iron greys are bright on purpose: on a metal slot COLOR_0 drives
# F0 (mirror tint), not diffuse, so darkening toward dielectric would be wrong.
WOOD = (0.46, 0.30, 0.16)          # warm oak haft (same as deployables)
WOOD_DK = (0.30, 0.19, 0.10)       # shadowed / grip-end
WOOD_LT = (0.55, 0.38, 0.22)       # raised knot / lit face
WRAP = (0.34, 0.22, 0.12)          # leather / twine wrap on the sword grip
WRAP_DK = (0.22, 0.14, 0.08)

STONE = (0.44, 0.40, 0.35)         # knapped stone point (warm grey)
STONE_DK = (0.30, 0.27, 0.23)
STONE_LT = (0.56, 0.52, 0.46)      # fresh chipped facet

IRON = (0.62, 0.63, 0.66)          # blade steel (bright; drives F0 on metal slot)
IRON_DK = (0.42, 0.43, 0.46)       # bevel / fuller shadow
IRON_LT = (0.82, 0.84, 0.88)       # edge highlight
IRON_GUARD = (0.50, 0.50, 0.52)    # crossguard / pommel / mace flanges (duller)

# Cord family (bow/crossbow strings). The Rust side binds a pale waxy-cord "Cord"
# material to any slot named *_string; the COLOR_0 here is the tint it modulates.
# The bow gets a pale tan waxed linen so the string reads as cord, clearly NOT
# wood-brown (the old bug: the string rode the WOOD material and looked wooden).
CORD_PALE = (0.72, 0.64, 0.48)     # pale tan / off-white waxed linen bowstring
# The crossbow keeps a darker waxed-cord tint: on its own Cord slot a dim tone
# reads as heavy tarred bowstring next to the bright iron prod, so string and
# limbs never merge into one grey mass.
CORD_DK = (0.26, 0.22, 0.16)       # dark tarred waxed cord (crossbow string)


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
    index so the export splits into one primitive per slot. Each piece is
    recalc'd on its own faces only, then joined.

    Most weapons use the classic 2-slot split (0 = grip, 1 = head). The bow and
    crossbow instead declare MANY named slots (one per animatable piece) via
    build_one's SLOTS table, because the Rust viewmodel animates each piece with
    its own rigid transform about a hardcoded pivot. Splitting each movable piece
    onto its own material slot is what makes it a separately addressable glTF
    primitive: primitives share the mesh node, so every piece stays in the same
    model space and the animator rotates a piece's verts about its documented
    pivot coordinate (translate(pivot) * rot * translate(-pivot))."""

    def __init__(self):
        self.bm = bmesh.new()
        self.col = self.bm.loops.layers.float_color.new("Color")
        self.uv = self.bm.loops.layers.uv.new("UVMap")

    def _finish_piece(self, faces, color_of, smooth):
        bmesh.ops.recalc_face_normals(self.bm, faces=faces)
        self.bm.normal_update()
        for f in faces:
            f.smooth = smooth
            for lp in f.loops:
                lp[self.col] = (*color_of(lp.vert), 1.0)
                lp[self.uv].uv = box_uv(lp.vert.co, f.normal)

    def _tag(self, faces, mat_index):
        for f in faces:
            f.material_index = mat_index

    def add_prism(self, ring_a, ring_b, color_of, mat_index, smooth=False,
                  cap_a=True, cap_b=True):
        """Bridge two equal-length vertex rings into a tube/frustum. Rings are
        lists of world-space (x,y,z). Optional end caps close the manifold."""
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

    def add_stack(self, rings, color_of, mat_index, smooth=False,
                  cap_first=True, cap_last=True):
        """Bridge a whole list of equal-length rings into ONE closed piece with a
        single recalc. Building a profile as one manifold (instead of stacked
        open bands) keeps recalc_face_normals reliable: on an open band it can
        orient an end cap inward, which backface-culls into a see-through pale
        notch at the pommel (the revision-round bug)."""
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

    def add_lathe(self, profile, sides, color_of, mat_index, smooth=False,
                  oval=1.0, jitter=0.0, seed=0.0):
        """A closed lathe: profile = [(z, radius), ...] swept into an n-gon tube
        capped at both ends. `oval` squashes the rings in Y; `jitter` roughens
        the per-ring radius deterministically for a gnarled read (one radius per
        ring, so adjacent bands always share coordinates and never crack)."""
        rings = []
        for k, (z, r) in enumerate(profile):
            rr = r * (1.0 + jitter * (hash01(k + seed, 1.0) - 0.5) * 2.0) if jitter else r
            rings.append(ngon_ring(0, 0, z, rr, rr * oval, sides))
        return self.add_stack(rings, color_of, mat_index, smooth)

    def add_knob(self, center, radius, color_of, mat_index):
        """A small faceted sphere knob (icosphere subdiv 1) embedded partway into
        a parent surface so it reads as a gnarl growing out of the mass."""
        res = bmesh.ops.create_icosphere(self.bm, subdivisions=1, radius=radius)
        verts = res["verts"]
        cx, cy, cz = center
        for v in verts:
            v.co.x += cx
            v.co.y += cy
            v.co.z += cz
        faces = set()
        for v in verts:
            for f in v.link_faces:
                faces.add(f)
        faces = list(faces)
        self._tag(faces, mat_index)
        self._finish_piece(faces, color_of, smooth=False)
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


# ============================================================ WOODEN CLUB =========
def build_club(b):
    """Thick knobbed hardwood club: tapered thin toward the grip, swelling into a
    heavy gnarled head mass at the top, with subtle wrap ridges on the handle.
    All wood, but split grip (prim 0) vs head mass (prim 1) so the two read as
    distinct volumes and the head can carry a slightly darker worn tone. The
    gnarl knobs are small faceted spheres embedded around the head's belly."""
    GRIP_BOT = -0.50
    GRIP_TOP = 0.02          # grip / head seam
    HEAD_TOP = 0.34
    SIDES = 8

    def grip_color(v):
        # darker at the very butt, wrap ridges give faint banding
        t = (v.co.z - GRIP_BOT) / (GRIP_TOP - GRIP_BOT)
        band = 0.5 + 0.5 * math.sin(v.co.z * 90.0)     # subtle ridge banding
        return lerp3(WOOD_DK, WOOD, 0.35 + 0.5 * t) if band > 0.6 else \
            lerp3(WOOD_DK, WOOD, 0.25 + 0.45 * t)

    # Handle: one closed lathe, thin near the butt, swelling to the seam, with a
    # gentle pommel knob so it does not slip from the hand.
    handle_profile = [
        (GRIP_BOT, 0.028),           # butt
        (GRIP_BOT + 0.03, 0.040),    # pommel knob
        (GRIP_BOT + 0.06, 0.030),
        (-0.30, 0.033),
        (-0.12, 0.038),
        (GRIP_TOP, 0.052),           # thickening into the head
    ]
    b.add_lathe(handle_profile, SIDES, grip_color, 0, smooth=True)

    # Head mass: a chunky swelling barrel, wider than tall, slightly jittered per
    # ring so it reads gnarled, not lathe-turned.
    def head_color(v):
        t = (v.co.z - GRIP_TOP) / (HEAD_TOP - GRIP_TOP)
        return lerp3(WOOD, WOOD_LT, 0.3 + 0.4 * t)

    head_profile = [
        (GRIP_TOP, 0.052),
        (GRIP_TOP + 0.04, 0.085),    # shoulder
        (0.14, 0.105),               # belly (widest)
        (0.24, 0.100),
        (HEAD_TOP - 0.02, 0.070),
        (HEAD_TOP, 0.040),           # rounded crown
    ]
    b.add_lathe(head_profile, SIDES, head_color, 1, smooth=False,
                jitter=0.05, seed=3.0)

    def head_r(z):
        """Un-jittered head surface radius at height z (for knob seating)."""
        for (z0, r0), (z1, r1) in zip(head_profile, head_profile[1:]):
            if z0 <= z <= z1:
                t = (z - z0) / max(z1 - z0, 1e-6)
                return r0 + (r1 - r0) * t
        return head_profile[-1][1]

    # Gnarl knobs: 7 small faceted spheres scattered around the belly at varied
    # heights and azimuths, each embedded most of the way into the mass so they
    # read as grown wood knots, not boxy ears stuck on top.
    for k in range(7):
        a = 2.0 * math.pi * k / 7 + 0.9 * hash01(k + 1.0, 3.0)
        z = 0.06 + 0.20 * hash01(k + 1.0, 5.0)          # belly band 0.06..0.26
        kr = 0.020 + 0.012 * hash01(k + 1.0, 7.0)       # knob radius 0.020..0.032
        d = head_r(z) + kr * 0.10                       # ~55% pokes out
        b.add_knob((math.cos(a) * d, math.sin(a) * d, z), kr, head_color, 1)


# ============================================================ STONE SPEAR =========
def build_spear(b):
    """Long straight wood haft with lashing ridges near the top, capped by a
    broad knapped stone leaf point. Reach weapon: the head runs taller than the
    other weapons (up to ~+0.65) but the grip section stays put so the hand pose
    matches. prim 0 wood haft, prim 1 stone point."""
    HAFT_BOT = -0.50
    HAFT_TOP = 0.42          # where the stone point sockets on
    POINT_TOP = 0.65
    SIDES = 7

    def haft_color(v):
        t = (v.co.z - HAFT_BOT) / (HAFT_TOP - HAFT_BOT)
        return lerp3(WOOD_DK, WOOD, 0.4 + 0.4 * t)

    # Straight slender haft, barely tapered, subtle grip swell low down. One
    # closed lathe so the butt cap winds outward (see add_stack).
    haft_profile = [
        (HAFT_BOT, 0.026),
        (HAFT_BOT + 0.02, 0.032),    # butt cap
        (HAFT_BOT + 0.05, 0.026),
        (-0.20, 0.025),
        (0.20, 0.024),
        (HAFT_TOP, 0.026),
    ]
    b.add_lathe(haft_profile, SIDES, haft_color, 0, smooth=True)

    # Lashing ridges near the top: a few fatter twine-coloured bands where the
    # point is bound to the haft. Each band is a CLOSED lathe whose end rings
    # sit at a radius just inside the haft surface, so the band visibly emerges
    # from the wood. The old open tubes (uncapped, floating 9 mm proud of the
    # shaft) read as detached rings hovering around the haft (owner report).
    def lash_color(v):
        return WOOD_DK
    for k in range(3):
        z = HAFT_TOP - 0.02 - k * 0.035
        lash_profile = [
            (z - 0.016, 0.023),   # buried inside the haft (r ~0.025 here)
            (z - 0.008, 0.034),   # proud band shoulder
            (z + 0.008, 0.034),
            (z + 0.016, 0.023),   # buried again
        ]
        b.add_lathe(lash_profile, SIDES, lash_color, 0, smooth=True)

    # Broad knapped stone leaf point: a flat faceted lozenge running up +X-thin,
    # widest at mid-length, tapering to a sharp tip. Built as two stacked flat
    # prisms (socket base -> belly -> tip) so it reads as a chipped blade.
    def point_color(v):
        # brighter fresh chips near the edges (|x| large), darker spine
        edge = min(1.0, abs(v.co.x) / 0.075)
        return lerp3(STONE, STONE_LT, 0.3 * edge)

    # flat leaf profile in the XY plane (X = width across the blade faces the
    # swing, Y = thin blade thickness).
    def leaf_ring(z, halfw, halfthick):
        return [(halfw, 0, z), (0, halfthick, z), (-halfw, 0, z), (0, -halfthick, z)]

    socket_z = HAFT_TOP - 0.01
    shoulder_z = socket_z + 0.05     # flares out fast into the broad leaf
    belly_z = 0.51                   # widest, lower so the leaf reads full
    tip_z = POINT_TOP
    # socket (narrow) -> quick flare -> broad belly -> long taper to a sharp tip,
    # so the point reads as a knapped LEAF (broad shoulders) not a slim diamond.
    b.add_prism(leaf_ring(socket_z, 0.024, 0.018), leaf_ring(shoulder_z, 0.11, 0.030),
                point_color, 1, smooth=False)
    b.add_prism(leaf_ring(shoulder_z, 0.11, 0.030), leaf_ring(belly_z, 0.12, 0.030),
                point_color, 1, smooth=False, cap_a=False, cap_b=False)
    b.add_prism(leaf_ring(belly_z, 0.12, 0.030), leaf_ring(tip_z, 0.004, 0.004),
                point_color, 1, smooth=False, cap_a=False, cap_b=True)


# ============================================================ IRON SWORD ==========
def build_sword(b):
    """Broad straight anime-proportioned blade with a simple crossguard and a
    faceted octagon disc pommel, everything coaxial on the Z axis. prim 0 =
    wrapped grip (wood family), prim 1 = blade + guard + pommel (iron family).
    Blade faces +X (into the swing), flat in Y. The blade root sinks INTO the
    guard box (no gap: a visible seam between them reads as an offset in the
    3/4 view) and the blade tapers steadily toward the tip so it reads forged,
    not slab-cut, while keeping the chunky anime width."""
    GRIP_TOP = -0.28         # under the guard
    GUARD_Z = -0.26          # guard box centre; box spans -0.288..-0.232
    BLADE_BOT = -0.285       # blade root buried inside the guard
    BLADE_TOP = 0.35
    SIDES = 6

    # --- grip (prim 0, wood/wrap): one closed oval lathe, coaxial ---
    def grip_color(v):
        band = 0.5 + 0.5 * math.sin(v.co.z * 120.0)   # wrap banding
        return WRAP if band > 0.5 else WRAP_DK

    grip_profile = [
        (-0.50, 0.026),      # buried inside the pommel disc
        (-0.42, 0.024),
        (-0.34, 0.026),
        (GRIP_TOP, 0.024),   # buried inside the guard
    ]
    b.add_lathe(grip_profile, SIDES, grip_color, 0, smooth=True, oval=0.72)

    def iron_color(v):
        # brighter toward the cutting edges (|x| large), darker toward the spine
        edge = min(1.0, abs(v.co.x) / 0.10)
        return lerp3(IRON_DK, IRON, 0.4 + 0.6 * edge)

    def guard_color(v):
        return IRON_GUARD

    # --- pommel (prim 1, iron): a flattened faceted octagon disc, coaxial ---
    pommel_profile = [
        (-0.530, 0.020),
        (-0.524, 0.044),
        (-0.498, 0.046),
        (-0.490, 0.022),
    ]
    b.add_lathe(pommel_profile, 8, guard_color, 1, smooth=False)

    # --- crossguard (prim 1, iron): a wide flat bar across X, centred on the
    # axis and wide enough to clear the blade shoulders on both sides ---
    b.add_box((0, 0, GUARD_Z), (0.150, 0.026, 0.028), guard_color, 1, smooth=False)

    # --- blade (prim 1, iron): one closed stack, root -> shoulder -> belly ->
    # taper -> tip. Hexagonal cross-section (flat faces + edge bevels). ---
    def blade_ring(z, halfw, halfthick):
        return [(halfw, 0, z), (halfw * 0.5, halfthick, z), (-halfw * 0.5, halfthick, z),
                (-halfw, 0, z), (-halfw * 0.5, -halfthick, z), (halfw * 0.5, -halfthick, z)]

    b.add_stack([
        blade_ring(BLADE_BOT, 0.048, 0.018),        # root, inside the guard
        blade_ring(-0.20, 0.088, 0.024),            # broad shoulder
        blade_ring(0.02, 0.078, 0.022),             # belly, already narrowing
        blade_ring(0.24, 0.052, 0.016),             # steady taper
        blade_ring(BLADE_TOP, 0.005, 0.004),        # point
    ], iron_color, 1, smooth=False)


# ============================================================ IRON MACE ===========
def build_mace(b):
    """Slender wood haft capped by a flanged iron head (5 flanges). prim 0
    haft, prim 1 head. Slimmed from the original stout proportions (owner
    feedback: both the handle and the head read too chunky); the head still
    carries clear mass over the haft so the slow committed swing reads."""
    HAFT_BOT = -0.50
    HAFT_TOP = 0.16          # head seats here
    HEAD_TOP = 0.35
    FLANGES = 5
    SIDES = 8

    def haft_color(v):
        t = (v.co.z - HAFT_BOT) / (HAFT_TOP - HAFT_BOT)
        return lerp3(WOOD_DK, WOOD, 0.4 + 0.4 * t)

    # Slender, near-straight haft with a small pommel knob. One closed lathe so
    # the butt cap winds outward (see add_stack).
    haft_profile = [
        (HAFT_BOT, 0.024),
        (HAFT_BOT + 0.03, 0.034),    # pommel knob
        (HAFT_BOT + 0.06, 0.026),
        (-0.10, 0.026),
        (HAFT_TOP, 0.028),
    ]
    b.add_lathe(haft_profile, SIDES, haft_color, 0, smooth=True)

    # Central iron ball core: ONE closed stack, capped both ends. The previous
    # build stacked four separate prisms (the middle two open bands); recalc on
    # an open band can orient its walls INWARD, which backface-culled the lower
    # core so the head's underside read as see-through into its own interior
    # (owner report). One manifold makes the normal orientation deterministic
    # (the same fix add_stack documents for the pommel).
    def core_color(v):
        return IRON_GUARD
    core_z = 0.255
    core_bot = HAFT_TOP - 0.012
    core_top = core_z + 0.09
    core_rings = []
    for i in range(5):
        z = core_bot + (core_top - core_bot) * i / 4
        rr = 0.042 * math.sin(math.pi * (0.15 + 0.7 * i / 4)) + 0.024
        core_rings.append(ngon_ring(0, 0, z, rr, rr, SIDES))
    b.add_stack(core_rings, core_color, 1, smooth=True)

    # Thin radial blade flanges. Widths are kept well under the root
    # circumference share (five roots at r 0.040 must not touch): the earlier
    # chunky wedges were wider than their slice of the core, so adjacent
    # flanges interpenetrated into a jumbled clump of crisscrossing faces that
    # read as z-fighting artifacts (owner report). Roots stay buried just
    # inside the core so no seam gap shows.
    def flange_color(v):
        edge = min(1.0, (math.hypot(v.co.x, v.co.y)) / 0.09)
        return lerp3(IRON_DK, IRON_LT, edge)

    for k in range(FLANGES):
        a = 2.0 * math.pi * k / FLANGES
        c, s = math.cos(a), math.sin(a)
        rot = Matrix.Rotation(a, 4, Vector((0, 0, 1)))
        # a thin blade: inner (at core) narrow+tall, outer (tip) pointed
        inner_r, outer_r = 0.040, 0.088
        halfw_in, halfw_out = 0.012, 0.006
        h_in, h_out = 0.060, 0.036
        verts_local = [
            (inner_r, -halfw_in, core_z - h_in), (inner_r, halfw_in, core_z - h_in),
            (inner_r, halfw_in, core_z + h_in), (inner_r, -halfw_in, core_z + h_in),
            (outer_r, -halfw_out, core_z - h_out), (outer_r, halfw_out, core_z - h_out),
            (outer_r, halfw_out, core_z + h_out), (outer_r, -halfw_out, core_z + h_out),
        ]
        vs = []
        for (lx, ly, lz) in verts_local:
            p = rot @ Vector((lx, ly, lz - core_z))
            vs.append(b.bm.verts.new((p.x, p.y, p.z + core_z)))
        b.bm.verts.ensure_lookup_table()
        quads = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
                 (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
        faces = [b.bm.faces.new([vs[i] for i in q]) for q in quads]
        b._tag(faces, 1)
        b._finish_piece(faces, flange_color, smooth=False)

    # Rounded top cap knob on the head crown.
    b.add_box((0, 0, HEAD_TOP - 0.015), (0.024, 0.024, 0.018), core_color, 1,
              smooth=True)


# ---- shared helper for the ranged pieces: a swept box-section ribbon ------------
def sweep_box_section(b, path, halfs, color_of, mat_index, smooth=False,
                      cap_first=True, cap_last=True):
    """Sweep a rectangular cross-section along a 3D `path` (list of (x,y,z)). Each
    station carries a half-size (hy, hx_or_hz...) via `halfs` (list of (hu, hv)
    pairs, one per path point) where u is the in-plane 'width' axis and v the
    'thickness' axis, both perpendicular to the local tangent. Tangents are
    computed from neighbouring points; the section is oriented by projecting a
    reference up-vector. Returns one closed manifold (one recalc). Used for the
    bow stave, the bow/crossbow strings, and the arrow shaft is a plain lathe.

    Sections are built in the plane spanned by (side, up) where `up` is the world
    axis least aligned with the tangent, so a stave curving in the XZ plane keeps
    flat faces toward the camera."""
    pts = [Vector(p) for p in path]
    n = len(pts)
    rings = []
    for i in range(n):
        # local tangent
        if i == 0:
            tan = (pts[1] - pts[0])
        elif i == n - 1:
            tan = (pts[-1] - pts[-2])
        else:
            tan = (pts[i + 1] - pts[i - 1])
        tan = tan.normalized() if tan.length > 1e-9 else Vector((0, 0, 1))
        # reference up = world axis least parallel to the tangent
        ax = min(range(3), key=lambda k: abs(tan[k]))
        ref = Vector((1 if ax == 0 else 0, 1 if ax == 1 else 0, 1 if ax == 2 else 0))
        side = tan.cross(ref).normalized()
        up = side.cross(tan).normalized()
        hu, hv = halfs[i]
        c = pts[i]
        ring = [
            (c + side * hu + up * hv),
            (c - side * hu + up * hv),
            (c - side * hu - up * hv),
            (c + side * hu - up * hv),
        ]
        rings.append([(v.x, v.y, v.z) for v in ring])
    return b.add_stack(rings, color_of, mat_index, smooth,
                       cap_first=cap_first, cap_last=cap_last)


# ============================================================ WOODEN BOW ==========
# Bow slot layout (build_one wires these named slots to primitives, in order).
# Each MOVABLE piece is its own slot with its geometry authored so the Rust
# animator can spin it about a fixed pivot coordinate. See BOW_RIG below for the
# documented pivots and the draw motion.
BOW_SLOTS = [
    ("wooden_bow_grip", False),          # 0 static: grip stave section + wrap band
    ("wooden_bow_limb_upper", False),    # 1 flexes about the upper junction
    ("wooden_bow_limb_lower", False),    # 2 flexes about the lower junction
    ("wooden_bow_string_upper", False),  # 3 Cord: upper string leg (tip -> nock)
    ("wooden_bow_string_lower", False),  # 4 Cord: lower string leg (tip -> nock)
    ("wooden_bow_arrow", False),         # 5 nocked arrow: rides the string nock
]

TIP_Z = 0.45                 # limb tip height (+-)
JUNCTION_Z = 0.085           # grip/limb junction height (+-); the limb FLEX PIVOT
GRIP_HALF = JUNCTION_Z       # grip zone half-height along Z
# The limb geometry starts a little BELOW its junction (overlapping into the grip
# zone) so, once the wrap sleeve is widened to cover the junction, the flex hinge
# is hidden and the limb never separates from the grip at draw. The flex pivot
# stays at JUNCTION_Z, so this overlap simply rotates with the limb.
LIMB_ROOT_Z = 0.050          # limb root height (+-); < JUNCTION_Z (overlap)
# The wrap sleeve covers PAST the junctions (both signs) so the rotating limb
# roots stay buried under the static grip wrap, closing the draw-time junction gap.
WRAP_HALF = 0.130            # wrap sleeve half-height along Z (> JUNCTION_Z)


def bow_stave_x(z):
    """Stave centreline X at height z. Grip at (x=belly,z=0); the belly bows toward
    the shooter (-X) deepest at the grip, and each limb hooks back FORWARD (+X)
    near the tip so the string chord clears the grip. Symmetric top/bottom."""
    t = z / TIP_Z                       # -1..1 across the bow
    belly = -0.115 * (1.0 - t * t)      # deepest at grip, 0 at tips
    hook = 0.16 * (t * t) * (0.5 + 0.5 * t * t)   # forward hook near tips
    return belly + hook


# Normalized limb half-profile measured from a GENERATED REFERENCE image
# (side-view strung self bow, FLUX schnell) with OpenCV: alpha-silhouette
# column scan, both limbs folded into one half-profile, median thickness per
# station. u = |z|/tip, t = thickness/max. The shape it found: a slight waist
# right at the grip, the thickest wood a third of the way out the limb, and a
# strong taper to ~40% at the tips. The reference's thickness-to-length ratio
# (~0.06) is also what set BOW_DEPTH_MAX: the previous hand-tuned stave was
# nearly twice as thick for its length (owner report: much slimmer).
BOW_THICK_PROFILE = [
    (0.000, 0.88), (0.125, 0.91), (0.208, 0.94), (0.292, 1.00),
    (0.458, 1.00), (0.542, 0.94), (0.625, 0.88), (0.708, 0.79),
    (0.792, 0.71), (0.875, 0.59), (1.000, 0.41),
]
# Half-depth at the thickest station: reference ratio 0.06 x total length 0.90,
# halved (profile values are half-sizes).
BOW_DEPTH_MAX = 0.027


def bow_profile_t(u):
    """Piecewise-linear sample of BOW_THICK_PROFILE at u in [0, 1]."""
    u = max(0.0, min(1.0, u))
    for (u0, t0), (u1, t1) in zip(BOW_THICK_PROFILE, BOW_THICK_PROFILE[1:]):
        if u <= u1:
            f = 0.0 if u1 == u0 else (u - u0) / (u1 - u0)
            return t0 + (t1 - t0) * f
    return BOW_THICK_PROFILE[-1][1]


def bow_stave_half(z):
    """(depth in X, width in Y) half-sizes of the stave section at height z,
    driven by the reference-measured BOW_THICK_PROFILE: slightly waisted at the
    grip, fullest a third out the limb, tapering hard to the tips. Width rides
    the same profile at a slimmer fraction, so the side-resting arrow passes a
    slender handle."""
    t = bow_profile_t(abs(z) / TIP_Z)
    depth = BOW_DEPTH_MAX * t
    width = 0.62 * depth
    return (depth, width)


# Pivot / anchor coordinates shared by the builder AND documented in BOW_RIG so
# the Rust animator matches them exactly. All in authoring Blender coords (Z up,
# X forward). Exported +Y up, so authoring Z -> in-game Y, authoring X -> in-game
# -Z is handled by the engine; the pivots below are stated in AUTHORING space and
# the rig doc restates the axis mapping.
BOW_TOP_TIP = (bow_stave_x(TIP_Z), 0.0, TIP_Z)
BOW_BOT_TIP = (bow_stave_x(-TIP_Z), 0.0, -TIP_Z)
BOW_UP_JUNCTION = (bow_stave_x(JUNCTION_Z), 0.0, JUNCTION_Z)
BOW_LO_JUNCTION = (bow_stave_x(-JUNCTION_Z), 0.0, -JUNCTION_Z)
# Nock (string centre) at REST sits ON the straight tip-to-tip chord: both tips
# share x = bow_stave_x(+-TIP_Z), so the rest nock is at that same x, z=0. The
# two legs are then collinear (one taut line clearing the grip belly). At draw
# the nock pulls toward the archer (+X) and the legs form the deep V.
BOW_NOCK = (bow_stave_x(TIP_Z), 0.0, 0.0)


def _bow_limb(b, z0, z1, slot, color):
    """Sweep one limb of the stave from junction height z0 to tip height z1. The
    geometry lives in the shared model space; its intended PIVOT is the junction
    point (bow_stave_x(z0), 0, z0), documented in BOW_RIG. Extra stations near the
    junction so the flex bends smoothly, not as one hinge."""
    stations = 9
    path, halfs = [], []
    for i in range(stations):
        z = z0 + (z1 - z0) * i / (stations - 1)
        path.append((bow_stave_x(z), 0.0, z))
        halfs.append(bow_stave_half(z))
    sweep_box_section(b, path, halfs, color, slot, smooth=True)


def _bow_string_leg(b, tip, slot, color):
    """One string leg: a slim box-section cord from a limb `tip` to the central
    nock. Its intended PIVOT is the limb tip; at draw the nock end swings back
    toward the archer (see BOW_RIG). The cross-section is a SQUARE cord (equal
    half-widths on both perpendicular axes), so the string reads as a slim
    round-ish cord from ANY angle: thin edge-on at rest AND thin when the draw
    re-aims the leg toward the camera. The rest leg runs straight along the limb
    axis (constant X, so its length axis is model Y and its cross-section lives in
    X-Z); the Rust animator stretches the leg ALONG that length axis only (never a
    uniform scale), so the cord never fattens into a plank at full draw."""
    tipv, nockv = Vector(tip), Vector(BOW_NOCK)
    stations = 6
    spath, shalfs = [], []
    HALF = 0.006                            # slim SQUARE cord half-width (both axes)
    for i in range(stations):
        t = i / (stations - 1)
        p = tipv.lerp(nockv, t)
        spath.append((p.x, p.y, p.z))
        shalfs.append((HALF, HALF))         # square cord section, not a flat plank
    sweep_box_section(b, spath, shalfs, color, slot, smooth=False,
                      cap_first=True, cap_last=True)


def build_bow(b):
    """A recurve-ish self bow authored as SEPARATE animatable pieces so the Rust
    viewmodel can play a draw: the two limbs flex about their grip junctions and
    the two string legs swing back into a deep V toward the archer.

    Pieces (each its own material slot, see BOW_SLOTS):
      wooden_bow_grip        static grip stave section (|z|<=0.085) + wrap band
      wooden_bow_limb_upper  upper limb, pivot at the upper junction
      wooden_bow_limb_lower  lower limb, pivot at the lower junction
      wooden_bow_string_upper  Cord: upper limb tip -> nock, pivot at the tip
      wooden_bow_string_lower  Cord: lower limb tip -> nock, pivot at the tip

    Rest pose keeps the old recurve silhouette (belly bows to -X, tips hook +X, a
    straight taut string chord clearing the grip). The string is now SLIM and on
    its own Cord slot (pale waxed-linen tint), so it no longer reads as wood. The
    two string legs meet AT the nock so at rest they form one near-straight line;
    at draw each rotates about its limb tip and the shared nock end pulls back."""
    SIDES = 6

    def stave_color(v):
        t = min(1.0, abs(v.co.z) / TIP_Z)
        return lerp3(WOOD, WOOD_DK, 0.15 + 0.35 * t)

    def grip_color(v):
        return lerp3(WOOD, WOOD_DK, 0.30)

    # --- grip stave section (slot 0, static): the central |z|<=JUNCTION_Z chunk of
    # the stave. Authored in place; its pivot/origin is the grip centre (0,0,0). ---
    gstations = 7
    gpath, ghalfs = [], []
    for i in range(gstations):
        z = -JUNCTION_Z + 2.0 * JUNCTION_Z * i / (gstations - 1)
        gpath.append((bow_stave_x(z), 0.0, z))
        ghalfs.append(bow_stave_half(z))
    sweep_box_section(b, gpath, ghalfs, grip_color, 0, smooth=True)

    # --- limbs (slots 1, 2): flex about their junctions. Author from just INSIDE
    # the grip zone (LIMB_ROOT_Z, below the junction) out to the tip, so the limb
    # root overlaps into the static grip and its flex hinge is buried under the
    # widened wrap sleeve (below) instead of leaving a visible gap at the junction
    # when the limb rotates. The Rust flex pivot still sits at the junction
    # (JUNCTION_Z), so the overlap rotates with the limb and stays flush. ---
    _bow_limb(b, LIMB_ROOT_Z, TIP_Z, 1, stave_color)
    _bow_limb(b, -LIMB_ROOT_Z, -TIP_Z, 2, stave_color)

    # --- string legs (slots 3, 4, Cord): slim pale cord, tip -> nock. ---
    def cord_color(v):
        return CORD_PALE

    _bow_string_leg(b, BOW_TOP_TIP, 3, cord_color)
    _bow_string_leg(b, BOW_BOT_TIP, 4, cord_color)

    # --- grip wrap band (slot 0, static): a short faceted leather sleeve around
    # the grip zone. Same slot as the grip stave (both static, never animate). ---
    def wrap_color(v):
        band = 0.5 + 0.5 * math.sin(v.co.z * 120.0)
        return WRAP if band > 0.5 else WRAP_DK

    # The wrap sleeve runs PAST both junctions (WRAP_HALF > JUNCTION_Z) so the
    # rotating limb roots stay tucked inside it and no gap opens at the junction
    # when the limbs flex at draw. It follows the stave centreline in X across its
    # span so it hugs the curved grip rather than a straight cylinder. Slimmed
    # with the waisted grip (see bow_stave_half) so the side-resting arrow
    # passes beside a slender handle, not a fat sleeve.
    wrap_profile = [
        (-WRAP_HALF, 0.028),
        (-WRAP_HALF + 0.018, 0.035),
        (WRAP_HALF - 0.018, 0.035),
        (WRAP_HALF, 0.028),
    ]
    wrap_rings = [
        ngon_ring(bow_stave_x(z), 0, z, r, r * 0.62, SIDES) for (z, r) in wrap_profile
    ]
    b.add_stack(wrap_rings, wrap_color, 0, smooth=True)

    # --- nocked arrow (slot 5): a ready arrow nocked on the string, shaft
    # running down-range (-X) from the rest nock at BOW_NOCK. Authored AT the
    # rest nock; the Rust animator translates the whole piece with the drawn
    # nock so it slides back with the string, and at full draw its exposed tip
    # becomes the archer's aim reference. The shaft CANTS across the bow the
    # way a real arrow does on a primitive self bow: the nock sits centred on
    # the string, and the shaft angles over to PASS BESIDE the grip (the -Y
    # flank, the archer's side rest), clearing the wrap instead of tunnelling
    # through the wood. A self bow has no cut-out arrow window (that is a
    # modern riser feature); the side rest is the period-correct read. Mini
    # version of build_arrow: wood shaft, two vertical fletching vanes in the
    # string plane, flat knapped stone lozenge head. One slot, so COLOR_0
    # carries the wood/stone split. ---
    def arrow_shaft_color(v):
        t = (BOW_NOCK[0] - v.co.x) / 0.60
        return lerp3(WOOD_DK, WOOD, 0.4 + 0.35 * max(0.0, min(1.0, t)))

    ARROW_NOCK_X = BOW_NOCK[0]
    ARROW_HEAD_X = ARROW_NOCK_X - 0.60       # socket end of the stone head
    ARROW_TIP_X = ARROW_HEAD_X - 0.115       # sharp tip, well past the belly
    ARROW_NOCK_Y = -0.012                    # nock, snug against the string
    # ONE straight line from the nock past the waisted grip: the earlier
    # piecewise path bent visibly at the pass point (owner report). The slope
    # is exactly what clears the slimmed wrap beside the grip; the
    # reference-slimmed stave lets the arrow lie straighter than before.
    ARROW_SLOPE = 0.075                      # dy per -dx along the shaft

    def arrow_y(x):
        return ARROW_NOCK_Y - ARROW_SLOPE * (ARROW_NOCK_X - x)

    shaft_path = [
        (ARROW_NOCK_X + 0.012, arrow_y(ARROW_NOCK_X + 0.012), 0.0),
        (-0.115, arrow_y(-0.115), 0.0),
        (ARROW_HEAD_X + 0.01, arrow_y(ARROW_HEAD_X + 0.01), 0.0),
    ]
    sweep_box_section(b, shaft_path, [(0.009, 0.009)] * 3, arrow_shaft_color, 5,
                      smooth=True)

    # nock block at the butt, dark, straddling the string.
    b.add_box((ARROW_NOCK_X + 0.014, ARROW_NOCK_Y + 0.006, 0),
              (0.008, 0.011, 0.013), lambda v: WOOD_DK, 5, smooth=False)

    # two fletching vanes, standing vertically (+-Z, the string plane) so they
    # read side-on from the camera like the string does.
    def vane_color(v):
        return lerp3(WOOD_LT, WOOD, 0.4)
    for sz in (1.0, -1.0):
        b.add_box((ARROW_NOCK_X - 0.055, arrow_y(ARROW_NOCK_X - 0.055), sz * 0.024),
                  (0.038, 0.0035, 0.015), vane_color, 5, smooth=False)

    # flat knapped stone lozenge head: broad in Z (vertical, camera-facing in
    # the string plane), thin in Y, tapering to the tip. Diamond rings in the
    # Y-Z plane bridged along X, riding the canted shaft line.
    def arrow_head_color(v):
        edge = min(1.0, abs(v.co.z) / 0.036)
        return lerp3(STONE, STONE_LT, 0.3 * edge)

    def head_ring(x, halfz, halfy):
        yc = arrow_y(x)
        return [(x, yc, halfz), (x, yc + halfy, 0),
                (x, yc, -halfz), (x, yc - halfy, 0)]

    b.add_prism(head_ring(ARROW_HEAD_X + 0.005, 0.014, 0.010),
                head_ring(ARROW_HEAD_X - 0.035, 0.038, 0.013),
                arrow_head_color, 5, smooth=False)
    b.add_prism(head_ring(ARROW_HEAD_X - 0.035, 0.038, 0.013),
                head_ring(ARROW_TIP_X, 0.003, 0.003),
                arrow_head_color, 5, smooth=False, cap_a=False, cap_b=True)


# ---- BOW RIG SPEC (the numbers the Rust animator must match) --------------------
# All coordinates are AUTHORING Blender space (Z up, X forward toward the archer,
# Y the thin axis). Exported +Y up, so at runtime authoring (x,y,z) -> in-game
# (x, z, -y) under the standard export_yup mapping; state pivots in authoring
# space and convert once on the Rust side. draw in [0,1]: 0 = braced rest (the
# exported model.glb pose), 1 = full draw.
#
#  piece                    pivot (authoring x,y,z)         rest -> draw motion
#  -----------------------  ------------------------------  --------------------------
#  wooden_bow_grip          none (static)                   never moves
#  wooden_bow_limb_upper    BOW_UP_JUNCTION                 rotate about pivot Y axis
#                           (x=-0.1079, y=0, z=+0.085)      by -BOW_LIMB_FLEX*draw rad
#                                                           (tip swings toward -X,
#                                                            i.e. toward the target)
#  wooden_bow_limb_lower    BOW_LO_JUNCTION                 rotate about pivot Y axis
#                           (x=-0.1079, y=0, z=-0.085)      by +BOW_LIMB_FLEX*draw rad
#                                                           (mirror of the upper)
#  wooden_bow_string_upper  BOW_TOP_TIP  (moves w/ limb)    rotate about the (flexed)
#                           (x=+0.16, y=0, z=+0.45)         tip so its nock end reaches
#                                                           the drawn nock (below)
#  wooden_bow_string_lower  BOW_BOT_TIP  (moves w/ limb)    mirror of the upper leg
#                           (x=+0.16, y=0, z=-0.45)
#
# The NOCK (shared loose end of both string legs) is the anchor to drive:
#   draw=0: BOW_NOCK          = (x=+0.16, y=0, z=0)  (on the straight braced chord)
#   draw=1: BOW_NOCK_DRAWN    = (x=+0.42, y=0, z=0)  (pulled +0.26 toward the archer)
# Simplest rig: (1) flex each limb about its junction by BOW_LIMB_FLEX*draw; this
# carries each limb tip; (2) place the nock at lerp(BOW_NOCK, BOW_NOCK_DRAWN, draw);
# (3) for each string leg, orient it from its (now flexed) tip to that nock and
# scale its length to span the gap. The two legs then form the deep rearward V.
#
# wooden_bow_arrow (slot 5) is authored with its nock AT BOW_NOCK, shaft along -X.
# Drive it as a rigid translate by (drawn nock - rest nock) so it rides the string,
# and collapse its scale right after loose while the loosed arrow flies.
BOW_LIMB_FLEX = 0.24                       # radians of limb rotation at full draw
BOW_NOCK_DRAWN = (0.42, 0.0, 0.0)          # nock position at draw=1 (authoring)


# ============================================================ CROSSBOW ============
# Crossbow slot layout. The string is now its OWN slot (crossbow_string) so the
# Rust viewmodel can translate it between cocked and released. Stock stays wood,
# prod/limbs/stirrup/trigger stay iron, minus the string. See CROSSBOW_RIG.
CROSSBOW_SLOTS = [
    ("crossbow_stock", False),   # 0 wood: rail + groove strip (static)
    ("crossbow_iron", True),     # 1 iron: prod, limbs, lug, stirrup, sights, trigger (static)
    ("crossbow_string", False),  # 2 Cord: the string (+ nut block), animatable
    ("crossbow_bolt", False),    # 3 loaded bolt in the groove (shows when cocked)
]

# Crossbow axis convention (authoring Blender coords; exported +Y up):
#   Z = stock long axis. STOCK_BOT (-0.42) is the BUTT (against the shooter's
#       shoulder); STOCK_TOP (+0.30) is the MUZZLE. The bolt flies toward +Z.
#   After export_yup, authoring +Z -> in-game +Y. So the muzzle/forward is
#   in-game +Y: the Rust side orients the crossbow so +Y points where the bolt
#   goes. X = limb span (side to side); Y (authoring) = up off the rail's face.
#   The Rust whole-item rotation includes a half-turn roll about the stock so
#   authoring +Y renders UP in first person: the groove, string, nut, bolt,
#   and sights all live on +Y (the top face) and the trigger hangs below (-Y).
#   (Without the roll the crossbow rendered belly-up: string and groove
#   underneath, the trigger block looming on top as a bulky fake "sight",
#   owner report.)
CB_STOCK_BOT = -0.42
CB_STOCK_TOP = 0.30
CB_PROD_Z = 0.28             # prod/limb bar at the muzzle
CB_LIMB_SPAN = 0.30          # each limb reaches +-this in X (total ~0.6)
# String nut z positions. COCKED = string drawn back and latched (the exported
# REST pose, the iconic loaded crossbow). RELEASED = string snapped forward to
# rest against the prod. The Rust side lerps the nut (and the two legs' loose
# ends) between these along +Z; legs also flatten as the nut nears the prod.
CB_NUT_Z_COCKED = CB_PROD_Z - 0.165     # 0.115: latched behind the prod
CB_NUT_Z_RELEASED = CB_PROD_Z - 0.020   # 0.260: forward against the prod


def build_crossbow(b):
    """A horizontal iron-reinforced prod on a slim wooden stock rail. The STOCK
    runs along the frame's Z axis (muzzle toward +Z, matching held-item forward),
    total height ~0.75; the limbs span across X up to ~0.6 wide, clearly more than
    2x the stock width.

    Pieces (each its own material slot, see CROSSBOW_SLOTS):
      crossbow_stock   wood rail + top groove strip (static)
      crossbow_iron    iron prod + limbs + centre lug + stirrup ring + trigger lug
      crossbow_string  Cord: the two-leg string + its nut block (animatable)

    The exported REST pose is COCKED (string latched back to a nut on the rail),
    the iconic loaded read. The Rust side slides the string forward to RELEASED
    (against the prod) on fire and back to COCKED on reload. Silhouette: slim
    vertical rail, wide swept-back iron prod, a deep dark string chevron, a thin
    open stirrup ring past the muzzle, a trigger lug under the rail."""
    STOCK_BOT = CB_STOCK_BOT
    STOCK_TOP = CB_STOCK_TOP
    PROD_Z = CB_PROD_Z
    LIMB_SPAN = CB_LIMB_SPAN
    SIDES = 6

    # --- stock (prim 0, wood): a slim rectangular RAIL, near-constant slim section
    # with a slight flare only at the shoulder (butt) end. Roughly half the width
    # and depth of the first-round slab so the prod dominates the silhouette. ---
    def stock_color(v):
        t = (v.co.z - STOCK_BOT) / (STOCK_TOP - STOCK_BOT)
        return lerp3(WOOD_DK, WOOD, 0.35 + 0.45 * t)

    stock_path = [
        (0, 0, STOCK_BOT),
        (0, 0, STOCK_BOT + 0.08),
        (0, 0, -0.05),
        (0, 0, 0.12),
        (0, 0, STOCK_TOP),
    ]
    stock_halfs = [
        (0.034, 0.038),      # shoulder flare at the butt
        (0.026, 0.028),      # settles into the rail
        (0.024, 0.026),      # rail
        (0.023, 0.025),
        (0.022, 0.024),      # muzzle
    ]
    sweep_box_section(b, stock_path, stock_halfs, stock_color, 0, smooth=False)

    # groove hint on top: a thin darker inlaid strip running the muzzle half of the
    # rail along +Z, sitting just proud of the front (+Y) face so it reads as the
    # bolt channel. (Wood family, prim 0.)
    def groove_color(v):
        return WOOD_DK
    b.add_box((0, 0.027, 0.09), (0.007, 0.005, 0.18), groove_color, 0, smooth=False)

    # --- prod / bow limbs (prim 1, iron): a WIDE bar across X at the muzzle, thick
    # at the centre lug, tapering to the tips, swept BACK toward the shooter (-Z)
    # so the limbs read as tensioned spring steel, not a straight T-cap. ---
    def iron_color(v):
        return lerp3(IRON_DK, IRON, 0.5)

    limb_stations = 11
    lpath = []
    lhalfs = []
    for i in range(limb_stations):
        t = -1.0 + 2.0 * i / (limb_stations - 1)     # -1..1 across the bar
        x = t * LIMB_SPAN
        # slight forward bow (+Y) at centre plus a clear backward sweep of the
        # tips toward the shooter (-Z): tensioned limbs, not a bar. The centre
        # hump is kept LOW: the prod crosses right under the ADS sight line,
        # and the earlier taller crown rose into it and hid the front post
        # (owner report).
        y = 0.012 * (1.0 - t * t) + 0.030
        z = PROD_Z - 0.055 * (t * t)
        lpath.append((x, y, z))
        at = abs(t)
        thick = 0.026 - 0.017 * at                   # taper to the tips
        height = 0.026 - 0.012 * at
        lhalfs.append((thick, height))
    sweep_box_section(b, lpath, lhalfs, iron_color, 1, smooth=True)

    # central prod lug: an iron block clamping the limbs to the rail. Kept small
    # and LOW: at ADS the lug sits dead centre of the sight picture, and the
    # earlier taller block stacked with the nut into one bulky central
    # obstruction (owner report). It now barely crests the limbs.
    b.add_box((0, 0.014, PROD_Z), (0.030, 0.026, 0.032), iron_color, 1, smooth=False)

    # --- string (slot 2, Cord): a taut deep V from each limb tip back to a NUT
    # BLOCK on the rail (the COCKED rest pose). Its own slot so the Rust side can
    # slide it forward to RELEASED. Thick section (0.012) reads at 160px; a dark
    # waxed-cord COLOR_0 (CORD_DK) keeps it distinct from the bright iron prod.
    # The nut block rides WITH the string on the same slot (it is the moving part
    # the archer's fingers latch), and its own centre (nut) is the string pivot. ---
    def string_color(v):
        return CORD_DK

    nut_z = CB_NUT_Z_COCKED
    # nut block on the rail's front (+Y) face: a LOW profile that hugs the
    # wood. It sits close to the aiming eye, so the earlier taller block
    # loomed as a big rectangle dead centre of the ADS sight picture (owner
    # report); now the sight line passes just over it.
    b.add_box((0, 0.031, nut_z), (0.014, 0.009, 0.020), string_color, 2, smooth=False)

    tip_l = Vector(lpath[0])
    tip_r = Vector(lpath[-1])
    nut = Vector((0.0, 0.036, nut_z))

    def string_leg(a, c):
        sp, sh = [], []
        segs = 6
        for i in range(segs + 1):
            t = i / segs
            p = a.lerp(c, t)
            sp.append((p.x, p.y, p.z))
            sh.append((0.012, 0.012))                # thick enough to read at 160px
        sweep_box_section(b, sp, sh, string_color, 2, smooth=False)
    string_leg(tip_l, nut)
    string_leg(tip_r, nut)

    # --- stirrup ring past the muzzle (prim 1, iron): the foot loop for
    # cocking, a slim wire frame hanging on the UNDERSIDE of the muzzle (-Y,
    # which renders DOWN) where a real stirrup lives, out of the sight line.
    # Wire-thin bars + a wide opening so it reads as a ring, never a block. ---
    sy = -0.012                      # under the rail axis: the stirrup hangs low
    ring_r = 0.056                   # opening half-width
    sect = 0.0045                    # wire-thin bar section
    st_base = STOCK_TOP - 0.012      # buried into the muzzle tip
    st_crown = STOCK_TOP + 0.078
    st_mid = (st_base + st_crown) / 2
    st_half = (st_crown - st_base) / 2
    b.add_box((0, sy, st_base), (ring_r + sect, sect, sect), iron_color, 1)   # base
    b.add_box((-ring_r, sy, st_mid), (sect, sect, st_half), iron_color, 1)    # left
    b.add_box((ring_r, sy, st_mid), (sect, sect, st_half), iron_color, 1)     # right
    b.add_box((0, sy, st_crown), (ring_r + sect, sect, sect), iron_color, 1)  # crown

    # --- iron sights (prim 1, iron), on the TOP face (+Y): a rear notch just
    # ahead of the eye and a single front post at the muzzle, both wire-thin so
    # they frame the target instead of hiding it. Every piece is ROOTED: the
    # ears rise off a pedestal seated into the rail and the front post runs
    # all the way down into the muzzle (the first pass floated both above the
    # wood, owner report). The ear tops and the post top share one level sight
    # plane (authoring y 0.080) that clears the prod's centre crown, the lug,
    # and the loaded bolt head, so nothing up front obstructs the picture
    # (owner report). ---
    def trig_color(v):
        return IRON_GUARD
    # rear: pedestal seated into the rail + two tall thin ears with the notch.
    b.add_box((0, 0.031, -0.045), (0.014, 0.009, 0.008), trig_color, 1,
              smooth=False)
    for sx in (-1.0, 1.0):
        b.add_box((sx * 0.0095, 0.058, -0.045), (0.004, 0.022, 0.0035),
                  trig_color, 1, smooth=False)
    # front: one post rooted in the muzzle, top on the sight plane.
    b.add_box((0, 0.052, 0.292), (0.0035, 0.028, 0.0035), trig_color, 1,
              smooth=False)

    # --- trigger (prim 1, iron): a low housing tucked under the rail (-Y
    # renders DOWN) with a short angled tongue, where a trigger belongs. Both
    # overlap the rail's underside in Y and Z so no gap reads from any angle. ---
    b.add_box((0, -0.030, -0.06), (0.016, 0.022, 0.050), trig_color, 1, smooth=False)
    b.add_box((0, -0.062, -0.095), (0.009, 0.018, 0.018),
              trig_color, 1, smooth=False,
              rot=Matrix.Rotation(math.radians(28), 4, Vector((1, 0, 0))))

    # --- loaded bolt (slot 3): a stocky arrow lying in the groove, its nock
    # butted against the COCKED nut, head just past the prod. The Rust side
    # shows it only while the string is cocked (it vanishes on fire while the
    # real projectile flies, and seats back in as the reload crank finishes),
    # and slides it with the nut so bolt and string stay glued. Wood family
    # slot; COLOR_0 carries the wood/stone split. ---
    def bolt_shaft_color(v):
        return lerp3(WOOD_DK, WOOD, 0.55)

    BOLT_Y = 0.040                   # resting on the groove strip
    BOLT_NOCK_Z = nut_z + 0.020      # butted on the nut's front face
    BOLT_HEAD_Z = 0.305              # socket end of the head, at the prod
    BOLT_TIP_Z = 0.385               # sharp tip past the muzzle
    bolt_path = [
        (0, BOLT_Y, BOLT_NOCK_Z),
        (0, BOLT_Y, BOLT_NOCK_Z + 0.06),
        (0, BOLT_Y, BOLT_HEAD_Z + 0.005),
    ]
    sweep_box_section(b, bolt_path, [(0.0085, 0.0085)] * 3, bolt_shaft_color, 3,
                      smooth=True)

    # two small fletching vanes, lying flat (+-X) so they read from above.
    def bolt_vane_color(v):
        return lerp3(WOOD_LT, WOOD, 0.4)
    for sx in (1.0, -1.0):
        b.add_box((sx * 0.020, BOLT_Y, BOLT_NOCK_Z + 0.030),
                  (0.013, 0.003, 0.026), bolt_vane_color, 3, smooth=False)

    # flat knapped stone head: broad in X (visible from the aiming eye above),
    # thin in Y, tapering to the tip.
    def bolt_head_color(v):
        edge = min(1.0, abs(v.co.x) / 0.030)
        return lerp3(STONE, STONE_LT, 0.3 * edge)

    def bolt_ring(z, halfx, halfy):
        return [(halfx, BOLT_Y, z), (0, BOLT_Y + halfy, z),
                (-halfx, BOLT_Y, z), (0, BOLT_Y - halfy, z)]

    b.add_prism(bolt_ring(BOLT_HEAD_Z, 0.013, 0.009),
                bolt_ring(BOLT_HEAD_Z + 0.028, 0.032, 0.012),
                bolt_head_color, 3, smooth=False)
    b.add_prism(bolt_ring(BOLT_HEAD_Z + 0.028, 0.032, 0.012),
                bolt_ring(BOLT_TIP_Z, 0.003, 0.003),
                bolt_head_color, 3, smooth=False, cap_a=False, cap_b=True)


# ---- CROSSBOW RIG SPEC (the numbers the Rust animator must match) ---------------
# Authoring Blender space (Z = stock long axis, +Z = muzzle/forward = where the
# bolt flies; X = limb span; Y = up off the rail face). Exported +Y up, so the
# bolt direction authoring +Z maps to in-game +Y: orient the crossbow so its
# in-game +Y points down-range. cock in [0,1]: 1 = COCKED (the exported model.glb
# pose, string latched back), 0 = RELEASED (string forward against the prod).
#
# ONE moving primitive: crossbow_string (the two legs + the nut block, all on the
# Cord slot). Drive it as a rigid translate along +Z, from the cocked nut to the
# released nut, and re-aim each leg from its (fixed) limb-tip anchor to the moving
# nut so the chevron flattens as the nut approaches the prod.
#
#  piece            anchor / pivot (authoring x,y,z)     cocked <-> released motion
#  ---------------  ----------------------------------   --------------------------
#  crossbow_stock   none (static)                        never moves
#  crossbow_iron    none (static)                        never moves
#  crossbow_string  nut centre = (0, +0.036, z_nut)      translate the nut (and the
#                   left leg anchored at LIMB_TIP_L      legs' loose ends with it)
#                   = (-0.30, +0.030, +0.225),           in +Z from z_nut=0.115
#                   right leg at LIMB_TIP_R              (COCKED) to z_nut=0.260
#                   = (+0.30, +0.030, +0.225)            (RELEASED); leg fixed ends
#                                                        stay at the limb tips.
#  crossbow_bolt    authored nocked at the COCKED nut    same translate as the
#                                                        string; scaled to zero
#                                                        while not near-cocked
#                                                        (fired / mid-reload).
#
#   z_nut(cock) = lerp(CB_NUT_Z_RELEASED, CB_NUT_Z_COCKED, cock)
#               = lerp(0.260, 0.115, cock)
# RELOAD is the reverse (released -> cocked); FIRE is cocked -> released, fast.
# The exported string sits at z_nut = CB_NUT_Z_COCKED (0.115): cocked.
CB_LIMB_TIP_L = (-CB_LIMB_SPAN, 0.030, CB_PROD_Z - 0.055)   # (-0.30, 0.030, 0.225)
CB_LIMB_TIP_R = (CB_LIMB_SPAN, 0.030, CB_PROD_Z - 0.055)    # (+0.30, 0.030, 0.225)


# ============================================================ ARROW ===============
def build_arrow(b):
    """A single straight arrow: wood shaft, knapped stone broadhead at the tip, two
    faceted vanes of fletching near the nock. Long and thin, Z_min -0.35 (nock) to
    Z_max +0.35 (tip). prim 0 = shaft + fletching, prim 1 = stone head. Doubles as
    the world stuck-arrow and the flying projectile, so it must read clean from all
    angles: the head is a flat knapped lozenge (like the spear point, smaller), the
    fletching two thin angled vanes, the nock a small notch block."""
    NOCK_Z = -0.35
    HEAD_BOT = 0.24          # stone head sockets on here
    TIP_Z = 0.35
    SIDES = 6

    # --- shaft (prim 0, wood): a slender near-cylindrical lathe from nock to head
    # socket, with a faint nock swell at the butt. ---
    def shaft_color(v):
        t = (v.co.z - NOCK_Z) / (HEAD_BOT - NOCK_Z)
        return lerp3(WOOD_DK, WOOD, 0.4 + 0.35 * t)

    shaft_profile = [
        (NOCK_Z, 0.014),             # nock butt
        (NOCK_Z + 0.02, 0.017),      # small nock swell
        (NOCK_Z + 0.05, 0.013),
        (0.0, 0.012),
        (HEAD_BOT, 0.013),           # under the head
    ]
    b.add_lathe(shaft_profile, SIDES, shaft_color, 0, smooth=True)

    # nock notch: a tiny dark block at the very butt with the string groove read.
    def nock_color(v):
        return WOOD_DK
    b.add_box((0, 0, NOCK_Z - 0.006), (0.016, 0.006, 0.010), nock_color, 0, smooth=False)

    # --- fletching (prim 0, wood family): two thin faceted vanes near the nock,
    # angled in the XY so they read as feathers from any spin angle. Each vane is a
    # flat triangular-ish quad strip standing off the shaft. ---
    def vane_color(v):
        return lerp3(WOOD_LT, WOOD, 0.4)

    fl_bot = NOCK_Z + 0.05
    fl_top = NOCK_Z + 0.17
    for k in range(2):
        a = math.pi * k          # two vanes, opposite sides (0 and 180 deg)
        c, s = math.cos(a), math.sin(a)
        r_in = 0.012             # inner edge hugs the shaft
        r_out = 0.052            # outer edge of the feather
        # a thin wedge: low+short at the ends, tall in the middle, standing along
        # the (c,s) radial direction, thin in the perpendicular direction.
        perp = (-s, c)
        def V(rr, z, side):
            return (c * rr + perp[0] * side * 0.004,
                    s * rr + perp[1] * side * 0.004,
                    z)
        # outer profile sweeps up then back for a swept-feather look
        verts_local = [
            V(r_in, fl_bot, -1), V(r_in, fl_bot, 1),
            V(r_in, fl_top, 1), V(r_in, fl_top, -1),
            V(r_out, fl_bot + 0.03, -1), V(r_out, fl_bot + 0.03, 1),
            V(r_out, fl_top - 0.02, 1), V(r_out, fl_top - 0.02, -1),
        ]
        vs = [b.bm.verts.new(p) for p in verts_local]
        b.bm.verts.ensure_lookup_table()
        quads = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
                 (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
        faces = [b.bm.faces.new([vs[i] for i in q]) for q in quads]
        b._tag(faces, 0)
        b._finish_piece(faces, vane_color, smooth=False)

    # --- stone broadhead (prim 1): a flat knapped lozenge, widest just above the
    # socket, tapering to a sharp tip. Flat in Y (thin), broad in X (faces the
    # camera in the 3/4 view). Built as stacked flat rings like the spear point. ---
    def head_color(v):
        edge = min(1.0, abs(v.co.x) / 0.045)
        return lerp3(STONE, STONE_LT, 0.3 * edge)

    def head_ring(z, halfw, halfthick):
        return [(halfw, 0, z), (0, halfthick, z), (-halfw, 0, z), (0, -halfthick, z)]

    socket_z = HEAD_BOT - 0.005
    shoulder_z = HEAD_BOT + 0.03
    tip_z = TIP_Z
    b.add_prism(head_ring(socket_z, 0.016, 0.012),
                head_ring(shoulder_z, 0.048, 0.016),
                head_color, 1, smooth=False)
    b.add_prism(head_ring(shoulder_z, 0.048, 0.016),
                head_ring(tip_z, 0.003, 0.003),
                head_color, 1, smooth=False, cap_a=False, cap_b=True)


# ============================================================ EXPORT ==============
# The classic 2-slot split (0 = grip/wood, 1 = head). The bool is the metal flag
# for the head slot. The bow and crossbow override this with their own multi-slot
# tables (BOW_SLOTS / CROSSBOW_SLOTS), one slot per animatable piece.
GRIP_HEAD_WOOD = [("grip", False), ("head", False)]
GRIP_HEAD_IRON = [("grip", False), ("head", True)]

WEAPONS = {
    "club": ("wooden_club", build_club, GRIP_HEAD_WOOD),
    "spear": ("stone_spear", build_spear, GRIP_HEAD_WOOD),
    "sword": ("iron_sword", build_sword, GRIP_HEAD_IRON),
    "mace": ("iron_mace", build_mace, GRIP_HEAD_IRON),
    "bow": ("wooden_bow", build_bow, BOW_SLOTS),
    "crossbow": ("crossbow", build_crossbow, CROSSBOW_SLOTS),
    "arrow": ("arrow", build_arrow, GRIP_HEAD_WOOD),
}


def make_slot_material(name, is_metal):
    """A minimal placeholder material per slot. The engine replaces it at load;
    it exists only so export_materials='EXPORT' emits a distinct primitive per
    slot and wires COLOR_0 into Base Color."""
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


def build_one(key, out_path):
    item_id, fn, slots = WEAPONS[key]
    bpy.ops.wm.read_homefile(use_empty=True)
    b = Builder()
    fn(b)
    obj = b.to_object(item_id)
    # One material slot per entry in the weapon's SLOTS table, in order, so the
    # face material_index tags emit one glTF primitive per slot. Short names
    # (grip/head) get the item_id prefix; the bow/crossbow already give full
    # names (wooden_bow_limb_upper, crossbow_string, ...) so the Rust table can
    # map each animatable primitive by name.
    for name, is_metal in slots:
        mat_name = name if name.startswith(item_id) else f"{item_id}_{name}"
        obj.data.materials.append(make_slot_material(mat_name, is_metal=is_metal))
    # Export (fresh selection/active so bpy.context is valid).
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
        key = argv[0]
        out = argv[1] if len(argv) > 1 else os.path.join(
            ITEMS, WEAPONS[key][0], "model.glb")
        build_one(key, out)
    else:
        for key, entry in WEAPONS.items():
            item_id = entry[0]
            build_one(key, os.path.join(ITEMS, item_id, "model.glb"))


if __name__ == "__main__":
    main()
