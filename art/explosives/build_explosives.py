#!/usr/bin/env python3
"""Build the explosives (P6a) as two-primitive Blender glbs: chunky
flat/soft cel silhouettes authored at WORLD scale with a ground-sitting base
origin, so each mesh doubles as the held item AND the placed/thrown world object.

Four pieces, one glb per id at assets/items/<id>/model.glb:
  powder_keg    : squat staved barrel, two iron hoops, upright rope fuse from the
                  bung. prim0 = wooden staves (powder_keg_body),
                  prim1 = iron hoops + bung + fuse (powder_keg_iron).
  satchel_charge: upright cloth bundle with gathered folds, one leather strap +
                  chunky buckle, rope binding, short fuse. prim0 = cloth bundle
                  (satchel_charge_cloth), prim1 = strap + buckle + rope + fuse
                  (satchel_charge_strap).
  powder_bomb   : faceted sphere with a gathered cloth pucker knot at the top and
                  a short fuse out of the knot. prim0 = cloth sphere
                  (powder_bomb_cloth), prim1 = knot + fuse (powder_bomb_knot).
  ember_charge  : dark riveted iron cage (vertical bars + two rings + four feet)
                  packed with an inner faceted crystal mass bursting through the
                  gaps. prim0 = iron cage + feet (ember_charge_cage),
                  prim1 = crystal mass (ember_charge_crystal).

EMBER GLOW CONVENTION (originally from the retired build_ore.py ember type): the crystal
prim's COLOR_0 carries an ember-orange colour with VERTEX ALPHA 1.0 as a GLOW MASK;
the engine's ToonMaterial emissive path gates on alpha >= 0.5, so only the crystal
facets emit. The cage prim (and every non-ember piece) uses alpha 0.0, so the glow
path is inert there. This is the SAME convention the meteorite ore node uses.

TWO-PRIMITIVE SPLIT: like the weapons pipeline, each mesh is one mesh object with
TWO named material slots so the gltf exporter emits TWO primitives. The engine
overlays one material family per primitive. We therefore export with
export_materials='EXPORT' and a DISTINCT, NAMED material slot per primitive; 'NONE'
collapses the split and breaks per-layer material assignment (art-pipeline gotcha).

REFERENCE FRAME: authored Blender Z-up, base sitting on Z=0 (ground), exported +Y up
via export_yup=True. So in-game these sit on the ground with their base at Y=0 and
stand up along +Y. The fuse runs up +Z (in-game +Y); a slight forward lean is +X.

COLOR_0 albedos are LINEAR (docs/rendering-materials.md). Wood matches the deployable
oak; iron greys are bright on purpose (on a metal slot COLOR_0 drives F0, not
diffuse). Cloth uses the warm undyed canvas the padded armour paints. Every mesh
gets box/triplanar UVs (a toon material with no UVs renders invisible) and a COLOR_0
"Color" attribute set as the render colour index.

Run headless (all four):
  /Applications/Blender.app/Contents/MacOS/Blender -b -P art/explosives/build_explosives.py
Or one piece:
  ... -P art/explosives/build_explosives.py -- <keg|satchel|bomb|ember> [out.glb]

FULL PIPELINE: concept (ComfyUI) -> this script -> glb ->
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

TEXEL = 0.20   # metres per detail-texture tile (box projection); world-scale props

# ---- COLOR_0 palette (LINEAR albedos, see docs/rendering-materials.md) ---------
# Wood matches the deployable oak so the keg staves read as the same hardwood.
# Iron greys are bright on purpose (metal slot: COLOR_0 part-drives F0). Cloth is
# the warm undyed canvas of the padded armour. Rope/leather match the armour straps.
WOOD = (0.44, 0.28, 0.15)          # warm oak stave
WOOD_DK = (0.28, 0.17, 0.09)       # stave gap / shadowed edge
WOOD_LT = (0.55, 0.37, 0.21)       # lit stave crest
BUNG = (0.36, 0.23, 0.12)          # cork/wood bung plug

IRON = (0.58, 0.60, 0.64)          # bright buckle steel (drives F0 on metal slot)
IRON_DK = (0.38, 0.40, 0.44)       # steel shadow / recess
IRON_LT = (0.78, 0.80, 0.85)       # rivet / edge highlight
IRON_RIVET = (0.22, 0.23, 0.25)    # dark rivet dot
# Dark banded iron for the keg HOOPS: the concept hoops are near-black bands and
# their darkness is load-bearing for the "barrel" read (bright grey hoops washed
# toward white and the barrel read as a plain cask/mug). Kept dark in COLOR_0 so
# both the icon and the in-game toon read them as black hoops, not bright steel.
HOOP = (0.11, 0.115, 0.125)        # near-black iron hoop band
HOOP_LT = (0.30, 0.31, 0.33)       # hoop rivet nub (a touch brighter, still dark)

CLOTH = (0.46, 0.38, 0.26)         # warm undyed sackcloth bundle
CLOTH_DK = (0.32, 0.26, 0.17)      # fold shadow / gathered crease
CLOTH_LT = (0.57, 0.48, 0.34)      # raised fold crest / lit face

LEATHER = (0.26, 0.15, 0.08)       # dark leather strap
LEATHER_LT = (0.38, 0.23, 0.12)
ROPE = (0.50, 0.40, 0.22)          # tan hemp rope binding / fuse cord
ROPE_DK = (0.34, 0.27, 0.15)
FUSE_TIP = (0.14, 0.12, 0.10)      # charred fuse tip (dark, so the spark reads)

# Ember crystal (same saturated glowing orange the meteorite node uses; the colour
# double-drives the emissive tint in Rust, so keep it hot). Alpha 1.0 = GLOW.
CRYSTAL = (0.900, 0.320, 0.045)
CRYSTAL_HI = (1.000, 0.620, 0.140)
CAGE = (0.070, 0.070, 0.078)       # near-black riveted iron cage (dark like ember slag)
CAGE_DK = (0.040, 0.040, 0.046)
CAGE_RIVET = (0.150, 0.150, 0.165) # rivet highlight on the dark cage


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


def ngon_ring(cx, cy, z, rx, ry, sides, phase=0.0):
    """A regular n-gon ring centred at (cx,cy,z), radii (rx,ry)."""
    ring = []
    for i in range(sides):
        a = phase + 2.0 * math.pi * i / sides
        ring.append((cx + math.cos(a) * rx, cy + math.sin(a) * ry, z))
    return ring


class Builder:
    """Accumulates faceted pieces into one bmesh, tagging each with a material
    index (0 / 1) so the export splits into two primitives. Each piece is recalc'd
    on its own faces only, then joined. Carries a per-piece GLOW ALPHA written into
    COLOR_0's alpha channel (1.0 = emissive crystal glow mask, 0.0 = everything
    else), the ember convention inherited from the retired build_ore.py."""

    def __init__(self):
        self.bm = bmesh.new()
        self.col = self.bm.loops.layers.float_color.new("Color")
        self.uv = self.bm.loops.layers.uv.new("UVMap")

    def _finish_piece(self, faces, color_of, smooth, glow=0.0):
        bmesh.ops.recalc_face_normals(self.bm, faces=faces)
        self.bm.normal_update()
        for f in faces:
            f.smooth = smooth
            for lp in f.loops:
                lp[self.col] = (*color_of(lp.vert), glow)
                lp[self.uv].uv = box_uv(lp.vert.co, f.normal)

    def _tag(self, faces, mat_index):
        for f in faces:
            f.material_index = mat_index

    def add_prism(self, ring_a, ring_b, color_of, mat_index, smooth=False,
                  cap_a=True, cap_b=True, glow=0.0):
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
        self._finish_piece(faces, color_of, smooth, glow)
        return faces

    def add_box(self, center, half, color_of, mat_index, smooth=False, rot=None,
                glow=0.0):
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
        self._finish_piece(faces, color_of, smooth, glow)
        return faces

    def add_stack(self, rings, color_of, mat_index, smooth=False,
                  cap_first=True, cap_last=True, glow=0.0):
        """Bridge a whole list of equal-length rings into ONE closed piece with a
        single recalc (keeps recalc_face_normals reliable, see build_weapons.py)."""
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
        self._finish_piece(faces, color_of, smooth, glow)
        return faces

    def add_lathe(self, profile, sides, color_of, mat_index, smooth=False,
                  oval=1.0, jitter=0.0, seed=0.0, glow=0.0, phase=0.0):
        """A closed lathe: profile = [(z, radius), ...] swept into an n-gon tube."""
        rings = []
        for k, (z, r) in enumerate(profile):
            rr = r * (1.0 + jitter * (hash01(k + seed, 1.0) - 0.5) * 2.0) if jitter else r
            rings.append(ngon_ring(0, 0, z, rr, rr * oval, sides, phase))
        return self.add_stack(rings, color_of, mat_index, smooth, glow=glow)

    def add_icosphere(self, center, radius, color_of, mat_index, subdiv=1,
                      smooth=False, glow=0.0, squash=1.0):
        """A faceted icosphere (subdiv 1 = 80 faces) translated to center, optional
        Z squash. Used for the powder-bomb cloth sphere and cage crystal lumps."""
        res = bmesh.ops.create_icosphere(self.bm, subdivisions=subdiv, radius=radius)
        verts = res["verts"]
        cx, cy, cz = center
        for v in verts:
            v.co.x += cx
            v.co.y += cy
            v.co.z = v.co.z * squash + cz
        faces = set()
        for v in verts:
            for f in v.link_faces:
                faces.add(f)
        faces = list(faces)
        self._tag(faces, mat_index)
        self._finish_piece(faces, color_of, smooth, glow)
        return faces

    def add_crystal(self, base_center, height, base_r, lean_deg, lean_az, seed,
                    color_of, mat_index, glow=1.0, sides=5):
        """A tall faceted gem spike (pentagonal base -> gem-cut shoulder -> apex),
        leaning away from a cluster axis. Inherited from the retired build_ore.py so
        the ember charge's inner mass reads the same as the meteorite node. glow
        defaults to 1.0 (the emissive glow mask)."""
        bx, by, bz = base_center
        lean = math.radians(lean_deg)
        lean_axis = Vector((-math.sin(lean_az), math.cos(lean_az), 0.0))
        rot = Matrix.Rotation(lean, 4, lean_axis) if abs(lean_deg) > 1e-3 \
            else Matrix.Identity(4)
        twist = hash01(seed, 7.0) * math.tau
        ring = []
        for i in range(sides):
            a = twist + i / sides * math.tau
            jr = base_r * (0.86 + 0.28 * hash01(seed + i, 3.0))
            local = rot @ Vector((math.cos(a) * jr, math.sin(a) * jr, 0.0))
            ring.append(self.bm.verts.new((bx + local.x, by + local.y, bz + local.z)))
        sh = []
        sh_z = height * 0.78
        for i in range(sides):
            a = twist + i / sides * math.tau
            jr = base_r * 0.34 * (0.8 + 0.4 * hash01(seed + i, 5.0))
            local = rot @ Vector((math.cos(a) * jr, math.sin(a) * jr, sh_z))
            sh.append(self.bm.verts.new((bx + local.x, by + local.y, bz + local.z)))
        apex_local = rot @ Vector(((hash01(seed, 1.0) - 0.5) * base_r * 0.3,
                                   (hash01(seed, 2.0) - 0.5) * base_r * 0.3, height))
        apex = self.bm.verts.new((bx + apex_local.x, by + apex_local.y,
                                  bz + apex_local.z))
        self.bm.verts.ensure_lookup_table()
        faces = []
        for i in range(sides):
            j = (i + 1) % sides
            faces.append(self.bm.faces.new([ring[i], ring[j], sh[j], sh[i]]))
            faces.append(self.bm.faces.new([sh[i], sh[j], apex]))
        self._tag(faces, mat_index)
        self._finish_piece(faces, color_of, smooth=False, glow=glow)
        return faces

    def add_torus_ring(self, cz, ring_r, sect, color_of, mat_index, seg=12,
                       sides=6, smooth=True, glow=0.0, oval=1.0):
        """A horizontal torus lying in the XY plane at height cz (a hoop / cage
        ring). ring_r = major radius, sect = tube (minor) radius."""
        vrings = []
        for s in range(seg):
            a = 2.0 * math.pi * s / seg
            cx, cy = math.cos(a) * ring_r, math.sin(a) * ring_r * oval
            ring = []
            for i in range(sides):
                b = 2.0 * math.pi * i / sides
                # tube cross-section in the (radial, z) plane
                rr = ring_r + math.cos(b) * sect
                ring.append((math.cos(a) * rr, math.sin(a) * rr * oval,
                             cz + math.sin(b) * sect))
            vrings.append(ring)
        # bridge consecutive cross-sections, closing the loop
        vs = [[self.bm.verts.new(p) for p in ring] for ring in vrings]
        self.bm.verts.ensure_lookup_table()
        faces = []
        for si in range(seg):
            ra = vs[si]
            rb = vs[(si + 1) % seg]
            n = len(ra)
            for i in range(n):
                j = (i + 1) % n
                faces.append(self.bm.faces.new([ra[i], ra[j], rb[j], rb[i]]))
        self._tag(faces, mat_index)
        self._finish_piece(faces, color_of, smooth, glow)
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


# ---- shared: a short rope FUSE swept along a gently curving path ----------------
def add_fuse(b, base, mat_index, length=0.14, lean=0.05, curl=0.03, r=0.010,
             seg=8, color_of=None, tip_color=None):
    """A short rope fuse rising from `base` (x,y,z), leaning +X and curling, built
    as a swept box-ish tube (square cross-section) so it survives the icon
    downscale. Returns the fuse TIP coordinate (for the spark VFX anchor). The last
    two segments take the charred `tip_color` so the tip reads dark against a spark.
    """
    if color_of is None:
        def color_of(v):
            return ROPE
    bx, by, bz = base
    pts = []
    for i in range(seg + 1):
        t = i / seg
        # rise mostly in +Z, lean toward +X, curl back near the tip like the concepts
        x = bx + lean * t + curl * math.sin(t * math.pi * 1.1)
        y = by
        z = bz + length * t
        pts.append(Vector((x, y, z)))
    # sweep a small square section along the path
    rings = []
    n = len(pts)
    for i in range(n):
        if i == 0:
            tan = pts[1] - pts[0]
        elif i == n - 1:
            tan = pts[-1] - pts[-2]
        else:
            tan = pts[i + 1] - pts[i - 1]
        tan = tan.normalized() if tan.length > 1e-9 else Vector((0, 0, 1))
        ax = min(range(3), key=lambda k: abs(tan[k]))
        ref = Vector((1 if ax == 0 else 0, 1 if ax == 1 else 0, 1 if ax == 2 else 0))
        side = tan.cross(ref).normalized()
        up = side.cross(tan).normalized()
        rr = r * (1.0 - 0.35 * (i / n))   # taper slightly toward the tip
        c = pts[i]
        ring = [(c + side * rr + up * rr), (c - side * rr + up * rr),
                (c - side * rr - up * rr), (c + side * rr - up * rr)]
        rings.append([(v.x, v.y, v.z) for v in ring])

    tip_hi = n - 2   # last two rings get the charred tip colour

    def fuse_color(v):
        return color_of(v)
    # build in two stacks so we can recolour the tip; overlap one ring so no crack
    b.add_stack(rings[:tip_hi + 1], fuse_color, mat_index, smooth=True,
                cap_first=True, cap_last=False)
    if tip_color is not None:
        def tipc(v):
            return tip_color
        b.add_stack(rings[tip_hi:], tipc, mat_index, smooth=True,
                    cap_first=False, cap_last=True)
    else:
        b.add_stack(rings[tip_hi:], fuse_color, mat_index, smooth=True,
                    cap_first=False, cap_last=True)
    return (pts[-1].x, pts[-1].y, pts[-1].z)


# ============================================================ POWDER KEG ==========
def build_keg(b):
    """Squat staved barrel ~0.55 m tall, bulged (bilged) profile, two dark iron
    hoops, a short upright rope fuse from the top bung. prim0 = wooden staves body,
    prim1 = iron hoops + bung + fuse. Base sits on Z=0.

    Silhouette guard: it must NOT read as a mug. A barrel reads by the BILGE (fat
    belly, narrower top+bottom) plus the two hoops banding it; a mug is a straight
    cylinder with a handle. So we bulge the profile hard and keep both ends inset."""
    STAVES = 12          # staved barrel: 12 flat facets read as planks
    H = 0.55
    fuse_tip = [0, 0, 0]

    # --- staves body (prim0, wood): a bilged lathe, base at z=0, fat belly at
    # ~0.27, insets top and bottom. 12 sides so each facet reads as a plank; the
    # per-vertex colour darkens the stave GAPS (azimuthal banding) for plank read.
    def stave_color(v):
        # vertical lit->shadowed gradient plus a faint per-stave banding so gaps read
        t = v.co.z / H
        az = math.atan2(v.co.y, v.co.x)
        band = 0.5 + 0.5 * math.cos(az * STAVES)      # crest at facet centre
        base = lerp3(WOOD_DK, WOOD, 0.35 + 0.4 * t)
        return lerp3(base, WOOD_LT, 0.25 * band)

    keg_profile = [
        (0.00, 0.150),       # base rim (inset)
        (0.03, 0.178),       # flares out fast off the ground
        (0.14, 0.205),
        (0.27, 0.215),       # belly (widest -> the bilge that sells "barrel")
        (0.40, 0.205),
        (0.51, 0.178),
        (0.55, 0.150),       # top rim (inset, symmetric with the base)
    ]
    b.add_lathe(keg_profile, STAVES, stave_color, 0, smooth=False)

    # --- iron hoops (prim1): two near-black bands girdling the barrel, one below
    # the belly and one above, each a thin torus sitting proud of the staves. ---
    def hoop_color(v):
        return HOOP

    def hoop_r_at(z):
        for (z0, r0), (z1, r1) in zip(keg_profile, keg_profile[1:]):
            if z0 <= z <= z1:
                tt = (z - z0) / max(z1 - z0, 1e-6)
                return r0 + (r1 - r0) * tt
        return keg_profile[-1][1]

    for hz in (0.13, 0.42):
        rr = hoop_r_at(hz) + 0.006
        b.add_torus_ring(hz, rr, 0.018, hoop_color, 1, seg=STAVES, sides=6,
                         smooth=True)

    # rivet nubs on the hoops (kept in the dark hoop family so they read as studs
    # on a black band, not bright steel dots)
    def rivet_color(v):
        return HOOP_LT
    for hz in (0.13, 0.42):
        rr = hoop_r_at(hz) + 0.024
        for k in range(4):
            a = 2.0 * math.pi * k / 4 + (0.4 if hz > 0.3 else 0.0)
            b.add_icosphere((math.cos(a) * rr, math.sin(a) * rr, hz), 0.012,
                            rivet_color, 1, subdiv=0)

    # --- bung + fuse (prim1): a short bung plug at the top centre, with the rope
    # fuse rising out of it. The top rim is inset so the bung sits on a small
    # recessed lid; we cap the barrel top with a flat lid disc first. ---
    def lid_color(v):
        return WOOD_DK
    b.add_lathe([(H - 0.005, 0.150), (H, 0.150)], STAVES, lid_color, 0,
                smooth=False)  # thin top lid ring flush with the rim

    def bung_color(v):
        return BUNG
    bung_z = H
    b.add_lathe([(bung_z, 0.030), (bung_z + 0.028, 0.036), (bung_z + 0.045, 0.028)],
                8, bung_color, 1, smooth=False)

    fuse_base = (0.0, 0.0, bung_z + 0.045)

    def fuse_color(v):
        return ROPE
    fuse_tip = add_fuse(b, fuse_base, 1, length=0.16, lean=0.06, curl=0.035,
                        r=0.012, color_of=fuse_color, tip_color=FUSE_TIP)
    b._fuse_tip = fuse_tip


# ============================================================ SATCHEL CHARGE ======
def build_satchel(b):
    """Upright soft rectangular cloth bundle ~0.35 m with visible gathered folds,
    one leather strap with a chunky buckle across it, rope binding, short fuse.
    prim0 = cloth bundle, prim1 = strap + buckle + rope + fuse. Base on Z=0.

    Silhouette guard: it must NOT read as a loaf/bread. A loaf is a smooth rounded
    lump; a SATCHEL CHARGE reads by (a) the vertical STRAP + buckle crossing it,
    (b) the rope binding cinching the middle, (c) the gathered rope ears at the top,
    (d) the fuse. We keep the bundle a clearly UPRIGHT rectangular block (taller
    than wide), softly bevelled, with faceted fold creases, and lean hard on the
    strap/buckle/rope so the read is 'wrapped charge', not 'food'."""
    H = 0.35
    HALF_X = 0.115       # width (across the strap)
    HALF_Y = 0.090       # depth
    SIDES = 8

    # --- cloth bundle (prim0): an upright rounded block. Built as a vertical stack
    # of rounded rectangular rings (superellipse-ish octagon) that bulge at the
    # middle (stuffed) and pinch slightly at top+bottom, with per-ring size jitter
    # for a soft hand-packed read. Folds come from the facet count + colour creases.
    def cloth_color(v):
        t = v.co.z / H
        # vertical fold creases: darker in the valleys of a sine around the azimuth
        az = math.atan2(v.co.y, v.co.x)
        fold = 0.5 + 0.5 * math.cos(az * 5.0 + 0.6)
        base = lerp3(CLOTH_DK, CLOTH, 0.4 + 0.35 * t)
        return lerp3(base, CLOTH_LT, 0.3 * fold)

    def rrect_ring(z, sx, sy):
        # rounded-rectangle ring: 8 points, corners pulled in for a soft block
        pts = []
        for i in range(SIDES):
            a = 2.0 * math.pi * i / SIDES
            # superellipse exponent > 2 gives flatter faces + rounded corners
            ca, sa = math.cos(a), math.sin(a)
            ex = math.copysign(abs(ca) ** 0.7, ca)
            ey = math.copysign(abs(sa) ** 0.7, sa)
            pts.append((ex * sx, ey * sy, z))
        return pts

    rings = []
    prof = [
        (0.00, 0.72, 0.72),   # base (pinched, sits softly)
        (0.04, 0.94, 0.94),
        (0.12, 1.04, 1.04),   # stuffed bulge
        (0.20, 1.06, 1.06),   # widest (stuffed middle)
        (0.28, 0.98, 0.98),
        (0.33, 0.80, 0.80),   # gathered toward the neck
        (0.35, 0.60, 0.60),   # top gather (cloth pinched up to the tie)
    ]
    for (z, fx, fy) in prof:
        jitter = 1.0 + 0.04 * (hash01(z * 20, 3.0) - 0.5)
        rings.append(rrect_ring(z, HALF_X * fx * jitter, HALF_Y * fy * jitter))
    b.add_stack(rings, cloth_color, 0, smooth=True)

    # --- gathered rope tails at the top (prim1): two short cord tails flopping OUT
    # to the sides off the neck gather (like the concept's loose tie ends). Kept
    # short and low-arcing (not tall+stiff, which read as bare twigs) and clearly
    # sideways so they read as the tied-off ends of the binding, not antlers. ---
    def rope_color(v):
        return ROPE

    neck_z = H - 0.03
    for k, (ax, sgn) in enumerate([(-1, 1), (1, -1)]):
        base = (ax * 0.05, 0.0, neck_z)
        # flop OUT (big lean in X) and only slightly UP (short length), curling down
        add_fuse(b, base, 1, length=0.055, lean=ax * 0.10, curl=sgn * 0.02,
                 r=0.010, seg=6, color_of=rope_color)

    # --- rope binding (prim1): two horizontal rope bands cinching the bundle (a
    # low band and the neck gather band), thin tori sitting proud of the cloth. ---
    def rr_at(z):
        for (z0, fx0, _), (z1, fx1, _) in zip(prof, prof[1:]):
            if z0 <= z <= z1:
                tt = (z - z0) / max(z1 - z0, 1e-6)
                return HALF_X * (fx0 + (fx1 - fx0) * tt)
        return HALF_X * prof[-1][1]

    for bz in (0.08, 0.30):
        rr = rr_at(bz) + 0.006
        b.add_torus_ring(bz, rr, 0.011, rope_color, 1, seg=SIDES + 4, sides=5,
                         smooth=True, oval=(HALF_Y / HALF_X))

    # --- leather strap (prim1): one wide vertical strap running up the FRONT face
    # (+Y), draped over the top, so it reads as a shoulder-strap charge. Built as a
    # thin box hugging the front, plus a chunky buckle plate mid-height. ---
    def strap_color(v):
        return LEATHER

    # front strap: a thin slab standing just proud of the +Y face, full height,
    # slightly angled like the concept (diagonal across the bundle). Centre + half
    # chosen so the rotated slab's lowest corner stays >= z 0 (ground-sitting base).
    strap_rot = Matrix.Rotation(math.radians(11), 4, Vector((0, 1, 0)))
    b.add_box((0.015, HALF_Y + 0.012, 0.195), (0.028, 0.010, 0.19),
              strap_color, 1, smooth=False, rot=strap_rot)

    # a second short strap band low down (the horizontal keeper) so the strap reads
    # as a real buckled strap, not a painted stripe.
    b.add_box((0.0, HALF_Y + 0.006, 0.10), (HALF_X * 0.8, 0.008, 0.022),
              strap_color, 1, smooth=False)

    # --- chunky buckle (prim1, iron): a frame + tongue on the front strap, mid
    # height, clearly proud so it catches light. ---
    def buckle_color(v):
        return IRON
    bkl_z = 0.205
    bkl_y = HALF_Y + 0.026
    # buckle frame = four thin bars forming a square ring
    fr = 0.030   # frame half-size
    ft = 0.008   # bar thickness
    b.add_box((0.02, bkl_y, bkl_z + fr), (fr + ft, ft, ft), buckle_color, 1)   # top
    b.add_box((0.02, bkl_y, bkl_z - fr), (fr + ft, ft, ft), buckle_color, 1)   # bot
    b.add_box((0.02 - fr, bkl_y, bkl_z), (ft, ft, fr), buckle_color, 1)        # left
    b.add_box((0.02 + fr, bkl_y, bkl_z), (ft, ft, fr), buckle_color, 1)        # right
    # tongue (a small bar across the middle)
    b.add_box((0.02, bkl_y + 0.004, bkl_z), (0.026, 0.006, 0.006), buckle_color, 1)

    # --- fuse (prim1): a short rope fuse out of the top gather, offset from the ears
    def fuse_color(v):
        return ROPE
    fuse_base = (0.0, 0.0, H + 0.005)
    fuse_tip = add_fuse(b, fuse_base, 1, length=0.13, lean=0.03, curl=0.03,
                        r=0.011, color_of=fuse_color, tip_color=FUSE_TIP)
    b._fuse_tip = fuse_tip


# ============================================================ POWDER BOMB =========
def build_bomb(b):
    """Faceted sphere ~0.22 m with a gathered cloth pucker knot at the top and a
    short fuse out of the knot. prim0 = cloth sphere, prim1 = knot + fuse. Base on
    Z=0 (the sphere sits on the ground; the sphere bottom is the origin base).

    Silhouette guard: the FUSE must survive the 160px downscale, so it is thick and
    stands clear of the knot. The pucker knot at the top (a cinched neck + a little
    torus tie) is what makes this a cloth POWDER bomb, not a plain ball."""
    R = 0.11             # sphere radius -> ~0.22 m diameter
    cz = R               # centre height so the base sits on z=0
    SIDES = 10

    # --- cloth sphere (prim0): a faceted UV-sphere with slight vertical fold
    # creases + a gathered pinch toward the top neck. Built as a lathe of rings so
    # we control the top gather precisely (an icosphere can't pucker cleanly). ---
    def cloth_color(v):
        # radial fold creases + a soft top->bottom light gradient
        az = math.atan2(v.co.y, v.co.x)
        fold = 0.5 + 0.5 * math.cos(az * 6.0 + 0.4)
        t = (v.co.z) / (2 * R)
        base = lerp3(CLOTH_DK, CLOTH, 0.42 + 0.32 * t)
        return lerp3(base, CLOTH_LT, 0.28 * fold)

    # sphere profile as (z, radius) rings from bottom to a pinched top neck
    ring_zs = [
        (0.00, 0.06),        # small flat contact at the base (sits stable)
        (0.02, 0.55),
        (0.05, 0.82),
        (0.09, 0.98),
        (R,    1.00),        # equator (widest)
        (2 * R - 0.09, 0.96),
        (2 * R - 0.05, 0.80),
        (2 * R - 0.02, 0.52),
        (2 * R + 0.005, 0.30),   # gather begins (cloth cinched up)
        (2 * R + 0.03, 0.20),    # neck (pucker)
    ]
    rings = [ngon_ring(0, 0, z, R * f, R * f, SIDES) for (z, f) in ring_zs]
    b.add_stack(rings, cloth_color, 0, smooth=True)

    neck_z = 2 * R + 0.03

    # --- pucker knot (prim1): a small torus tie cinching the neck + a couple of
    # short gathered cloth folds fanning above it, like the concept. ---
    def knot_color(v):
        return ROPE_DK
    b.add_torus_ring(neck_z, R * 0.22, 0.016, knot_color, 1, seg=SIDES, sides=6,
                     smooth=True)

    # gathered cloth crown above the tie: a small pinched cone of cloth (the fabric
    # gathered up through the tie), coloured cloth so it reads as fabric, not rope.
    def crown_color(v):
        return CLOTH_DK
    b.add_lathe([(neck_z, R * 0.20), (neck_z + 0.02, R * 0.16),
                 (neck_z + 0.045, R * 0.10)], 6, crown_color, 1, smooth=False)

    # --- fuse (prim1): a short thick rope fuse out of the knot crown. Thick + a
    # gentle S-curl like the concept, tip dark so a spark reads at the end. ---
    def fuse_color(v):
        return ROPE
    fuse_base = (0.0, 0.0, neck_z + 0.045)
    fuse_tip = add_fuse(b, fuse_base, 1, length=0.12, lean=0.05, curl=0.045,
                        r=0.013, color_of=fuse_color, tip_color=FUSE_TIP)
    b._fuse_tip = fuse_tip


# ============================================================ EMBER CHARGE ========
def build_ember(b):
    """Dark riveted iron cage (vertical bars + two horizontal rings, four stubby
    feet) ~0.45 m, packed with an inner faceted crystal mass bursting slightly
    through the gaps. prim0 = iron cage + feet, prim1 = crystal mass.

    CRITICAL: the crystal prim carries the ember GLOW convention (COLOR_0 alpha 1.0,
    ember-orange colour) so the ToonMaterial emissive path lights it; the cage prim
    is alpha 0.0. Copied from art/ore/build_ore.py's ember type.

    Silhouette guard: it must read CAGED-GLOW. The cage bars are chunky and dark,
    spaced so the orange crystal is clearly visible BETWEEN them and a few crystal
    tips poke OUT past the bars/top (the 'bursting through' read from the concept).
    The four feet + top bung-cap match the concept's iron vessel."""
    H = 0.45
    BODY_BOT = 0.075     # cage body starts above the feet
    BODY_TOP = 0.40      # cage crown (below the top cap)
    R = 0.155            # cage radius (round-bellied like the concept)
    BARS = 8             # vertical cage bars
    CZ = (BODY_BOT + BODY_TOP) / 2

    # ---- inner crystal mass (prim1, GLOW alpha 1.0) built FIRST so it sits inside.
    # A central faceted lump + a fan of crystal spikes bursting upward and a few
    # poking sideways through the bar gaps. Copied convention from build_ore ember.
    def crystal_color(v):
        # brighter toward the tips/top so it reads lit from within
        t = max(0.0, min(1.0, (v.co.z - BODY_BOT) / (H - BODY_BOT)))
        return lerp3(CRYSTAL, CRYSTAL_HI, 0.25 + 0.5 * t)

    # central molten lump filling the belly
    b.add_icosphere((0, 0, CZ - 0.01), R * 0.72, crystal_color, 1, subdiv=1,
                    smooth=False, glow=1.0, squash=1.15)

    # erupting spike fan from the crown (some tall enough to clear the top cap)
    spikes = [
        (0,   0.62, 0.24, 0.052, 2),
        (45,  0.58, 0.20, 0.046, 12),
        (90,  0.60, 0.22, 0.048, -10),
        (135, 0.56, 0.18, 0.044, 14),
        (180, 0.60, 0.21, 0.048, -6),
        (225, 0.57, 0.19, 0.045, 10),
        (270, 0.59, 0.20, 0.046, -14),
        (315, 0.58, 0.19, 0.045, 8),
        (20,  0.70, 0.30, 0.055, 3),   # tall central-ish spikes bursting past the top
        (200, 0.68, 0.27, 0.052, -4),
    ]
    for k, (az, elev, sh_h, base_r, lean) in enumerate(spikes):
        a = math.radians(az)
        rr = R * 0.42 * (0.5 + 0.5 * elev)
        base = (math.cos(a) * rr, math.sin(a) * rr, BODY_BOT + elev * (H - BODY_BOT))
        b.add_crystal(base, sh_h, base_r, lean, a, seed=40.0 + k * 4,
                      color_of=crystal_color, mat_index=1, glow=1.0)

    # side-bursting shards poking OUT through the bar gaps (the 'bursting' read)
    for k in range(4):
        a = math.radians(22.5 + k * 90)   # aim between bars
        rr = R * 0.95
        base = (math.cos(a) * rr, math.sin(a) * rr, CZ + 0.02)
        # lean outward + up
        b.add_crystal(base, 0.10, 0.034, 34, a + math.pi, seed=70.0 + k * 5,
                      color_of=crystal_color, mat_index=1, glow=1.0)

    # ---- iron cage (prim0, alpha 0.0). Dark bars + two rings + feet + top cap. ----
    def cage_color(v):
        return CAGE

    def cage_lt(v):
        return CAGE_RIVET

    # two horizontal cage rings (belly + shoulder), chunky dark tori
    for rz in (BODY_BOT + 0.03, BODY_TOP - 0.03):
        b.add_torus_ring(rz, R, 0.018, cage_color, 0, seg=BARS * 2, sides=6,
                         smooth=True)
    # a mid ring too, for the round-bellied 3-band cage read
    b.add_torus_ring(CZ, R * 1.02, 0.020, cage_color, 0, seg=BARS * 2, sides=6,
                     smooth=True)

    # vertical bars: chunky square bars following the belly bulge (wider at CZ).
    for k in range(BARS):
        a = 2.0 * math.pi * k / BARS
        c, s = math.cos(a), math.sin(a)
        # each bar is a swept box from bottom ring to top ring, bulged at the belly
        rings = []
        for (z, rf) in [(BODY_BOT, 1.0), (CZ, 1.06), (BODY_TOP, 1.0)]:
            rr = R * rf
            cx, cy = c * rr, s * rr
            # square cross-section oriented tangent/radial
            tx, ty = -s, c   # tangent
            hw, hd = 0.016, 0.018   # bar half-width (tangent), half-depth (radial)
            ring = [
                (cx + tx * hw + c * hd, cy + ty * hw + s * hd, z),
                (cx - tx * hw + c * hd, cy - ty * hw + s * hd, z),
                (cx - tx * hw - c * hd, cy - ty * hw - s * hd, z),
                (cx + tx * hw - c * hd, cy + ty * hw - s * hd, z),
            ]
            rings.append(ring)
        b.add_stack(rings, cage_color, 0, smooth=False)

    # rivet nubs where bars meet rings (dark cage highlight dots)
    for rz in (BODY_BOT + 0.03, BODY_TOP - 0.03):
        for k in range(BARS):
            a = 2.0 * math.pi * k / BARS
            rr = R + 0.020
            b.add_icosphere((math.cos(a) * rr, math.sin(a) * rr, rz), 0.011,
                            cage_lt, 0, subdiv=0)

    # top cap (the iron bung/chimney on the concept): a short dark cylinder cap
    b.add_lathe([(BODY_TOP - 0.01, R * 0.55), (BODY_TOP + 0.02, R * 0.42),
                 (BODY_TOP + 0.02, R * 0.30)], BARS, cage_color, 0, smooth=True)
    b.add_lathe([(BODY_TOP + 0.02, 0.040), (BODY_TOP + 0.055, 0.044),
                 (BODY_TOP + 0.075, 0.036)], 8, cage_color, 0, smooth=False)

    # bottom base ring the cage sits on (closes the underside look)
    b.add_torus_ring(BODY_BOT, R * 0.9, 0.020, cage_color, 0, seg=BARS * 2, sides=6,
                     smooth=True)

    # four stubby feet splayed out (like the concept's iron feet)
    def foot_color(v):
        return CAGE_DK
    for k in range(4):
        a = math.radians(45 + k * 90)
        c, s = math.cos(a), math.sin(a)
        fx, fy = c * R * 0.72, s * R * 0.72
        # a short angled leg from the base ring down-out to a little pad on the ground
        top = Vector((fx, fy, BODY_BOT))
        pad = Vector((c * R * 1.02, s * R * 1.02, 0.0))
        # sweep a box leg
        legrings = []
        for t in (0.0, 1.0):
            p = top.lerp(pad, t)
            tx, ty = -s, c
            hw = 0.026 - 0.006 * t
            hd = 0.024 - 0.006 * t
            ring = [
                (p.x + tx * hw + c * hd, p.y + ty * hw + s * hd, p.z),
                (p.x - tx * hw + c * hd, p.y - ty * hw + s * hd, p.z),
                (p.x - tx * hw - c * hd, p.y - ty * hw - s * hd, p.z),
                (p.x + tx * hw - c * hd, p.y + ty * hw - s * hd, p.z),
            ]
            legrings.append(ring)
        b.add_stack(legrings, foot_color, 0, smooth=False)

    # The ember charge's "fuse tip" for the spark VFX is the tip of the tallest
    # crystal bursting out of the top (there is no rope fuse; the glow erupts). We
    # report the crown of the tallest central spike.
    tall = spikes[8]   # (20, 0.70, 0.30, ...) -> tallest
    a = math.radians(tall[0])
    rr = R * 0.42 * (0.5 + 0.5 * tall[1])
    bx, by = math.cos(a) * rr, math.sin(a) * rr
    bz = BODY_BOT + tall[1] * (H - BODY_BOT)
    b._fuse_tip = (bx, by, bz + tall[2])


# ============================================================ EXPORT ==============
EXPLOSIVES = {
    "keg":     ("powder_keg", build_keg,
                ("powder_keg_body", False), ("powder_keg_iron", True)),
    "satchel": ("satchel_charge", build_satchel,
                ("satchel_charge_cloth", False), ("satchel_charge_strap", False)),
    "bomb":    ("powder_bomb", build_bomb,
                ("powder_bomb_cloth", False), ("powder_bomb_knot", False)),
    "ember":   ("ember_charge", build_ember,
                ("ember_charge_cage", True), ("ember_charge_crystal", False)),
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
    bsdf.inputs["Roughness"].default_value = 0.45 if is_metal else 0.85
    vc = nt.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    m.use_backface_culling = True
    return m


def build_one(key, out_path):
    item_id, fn, (slot0_name, slot0_metal), (slot1_name, slot1_metal) = EXPLOSIVES[key]
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
    # export_vertex_color="NAME" (not "ACTIVE"): in Blender 5.x, with
    # export_materials="EXPORT" the ACTIVE/MATERIAL modes strip COLOR_0 to VEC3
    # (they only emit the alpha channel when the material's alpha input is driven by
    # it), which silently DROPS the ember GLOW MASK alpha. NAME mode dumps the named
    # attribute verbatim as VEC4, so alpha survives (matches the meteorite ore glb,
    # which gets VEC4 for free because build_ore.py exports with materials='NONE').
    bpy.ops.export_scene.gltf(
        filepath=out_path, export_format="GLB", use_selection=True,
        export_yup=True, export_apply=True, export_normals=True,
        export_materials="EXPORT", export_texcoords=True,
        export_vertex_color="NAME", export_vertex_color_name="Color",
    )
    ft = getattr(b, "_fuse_tip", None)
    print(f"EXPORTED {out_path}  FUSE_TIP(authoring Z-up)={ft}")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if argv:
        key = argv[0]
        out = argv[1] if len(argv) > 1 else os.path.join(
            ITEMS, EXPLOSIVES[key][0], "model.glb")
        build_one(key, out)
    else:
        for key, (item_id, *_rest) in EXPLOSIVES.items():
            build_one(key, os.path.join(ITEMS, item_id, "model.glb"))


if __name__ == "__main__":
    main()
