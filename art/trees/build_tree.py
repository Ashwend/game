#!/usr/bin/env python3
"""Build an Ashwend tree as a Blender glb: a textured bark TRUNK (round, tapered,
UV-wrapped, real smooth normals) + a textured FOLIAGE canopy (capless cone stack
for pine / leaf blobs for birch, tiled UV, UP-BIASED custom vertex normals so it
lights soft like foliage, bottom rim feathered via vertex-colour alpha + Mask).

Two separate objects -> glb mesh0 = trunk, mesh1 = foliage. Rust loads each mesh
and assigns its own shared StandardMaterial (bark opaque / foliage Mask).

Run headless:
  Blender --background --python build_tree.py -- <species> <size> <out.glb> [preview.png] [bark.png needles.png]
species: pine|birch   size: small|medium|large
Geometry constants mirror src/app/scene/mesh/trees.rs exactly (silhouette parity).

------------------------------------------------------------------------------
FULL PIPELINE (Draw Things -> OpenCV -> Blender), reproducible from scratch.
GEN = ~/.claude/skills/lowpoly-game-assets/scripts/generate.py (FLUX schnell).
FOL = art/textures/trees/make_foliage.py (OpenCV alpha/seamless/de-grey).
Masters of the chosen raw gens are kept beside this file (*_master.png).

1. Bark (opaque, tiled): seamless texture gens, pick best, tone down, 512:
   GEN texture --subject "pine conifer tree bark, deeply furrowed reddish-brown
       plates, vertical ridges" --extra "rugged organic bark grooves, weathered,
       naturalistic but stylized" --out pine_bark.png --size 512 --variants 3 --seed 7001
   magick <chosen> -resize 512x512 -modulate 96,82,100 -brightness-contrast -4x4 \
       assets/textures/trees/bark_pine.png
   GEN texture --subject "birch tree bark" --extra "pale warm grey-white papery
       bark, thin dark horizontal lenticel bands, smooth, no vertical furrows" \
       --out birch_bark.png --size 512 --variants 3 --seed 8100
   magick <chosen> -resize 512x512 -modulate 90,88,100 assets/textures/trees/bark_birch.png

2. Canopy (alpha-mask): raw foliage patch gens, then FOL keys greenness->alpha,
   cleans, RGB-bleeds, de-greys, makes seamless, 512:
   GEN texture --no-seamless --subject "dense evergreen pine needle sprigs" \
       --extra "clustered conifer needle fronds, muted sage and forest green,
       irregular gaps, plain pale grey background, no trunk, no sky" \
       --out needles_raw.png --size 1024 --variants 4 --seed 4200
   FOL --src needles_master.png --out assets/textures/trees/needles.png --thr 12 \
       --lum-floor 18 --degrey 0.28 --degrey-color 60 92 50 --sat 0.95 --val 0.96 --size 512
   GEN texture --no-seamless --subject "small rounded birch leaves" --extra
       "clustered deciduous foliage, soft yellow-green and olive, irregular leaf
       clumps with gaps, plain pale grey background, no trunk, no sky" \
       --out leaves_raw.png --size 1024 --variants 4 --seed 5300
   FOL --src leaves_master.png --out assets/textures/trees/leaves.png --thr 8 \
       --lum-floor 38 --hue 16 --degrey 0.18 --degrey-color 70 110 55 --sat 0.8 --val 0.95 --size 512

3. Models: for species in pine birch, size in small medium large:
   Blender --background --python art/trees/build_tree.py -- <species> <size> \
       assets/trees/<species>_<size>/model.glb "" \
       assets/textures/trees/bark_<species>.png assets/textures/trees/<needles|leaves>.png
   (export_materials='NONE' -> ~10 KB glbs; Rust builds the shared materials.)

4. cargo build re-embeds assets/; verify in-game with the headless harness.
"""
import bpy, bmesh, sys, math, os
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
SPECIES = argv[0] if len(argv) > 0 else "pine"
SIZE = argv[1] if len(argv) > 1 else "medium"
OUT = argv[2] if len(argv) > 2 else "/tmp/tree.glb"
PREVIEW = argv[3] if len(argv) > 3 else ""
BARK_TEX = argv[4] if len(argv) > 4 else ""
FOLIAGE_TEX = argv[5] if len(argv) > 5 else ""

RADIAL = 8                # trunk radial segments
FOLIAGE_TEXEL = 0.62      # metres per needle/leaf texture tile
BARK_V_TILE = 2.0         # metres per bark vertical tile
UP_BIAS = 0.72            # foliage normal lerp toward +Z (up)
FEATHER_FZ = 0.16         # fraction of cone height that feathers at the base
RIM_ALPHA = 0.0           # vertex alpha at the feathered base rim
# The trunk continues up through the canopy as a thin tapering spine to this
# fraction of the canopy top, so a real conifer/birch reads with a continuous
# trunk (visible through the foliage gaps) instead of a stub cut off where the
# branches start. The thin tip stays inside the top foliage layer.
TRUNK_EXTEND_FRAC = 0.93
TRUNK_TIP_R = 0.035

# ---- per-tree config (mirrors trees.rs) ---------------------------------------
# trunk: list of (z, radius). cones: (base_z, height, radius, segs, tone, xoff, yoff)
# blobs: (cx, cy, cz, sx, sy, sz, tone)   [birch octa-rock crown]
TONE = {"dark": 0.66, "mid": 0.84, "light": 1.0,
        "bdark": 0.66, "bmid": 0.85, "blight": 1.0}

CFG = {
 ("pine","small"): dict(
   trunk=[(0,0.22),(0.20,0.20),(0.65,0.16),(1.10,0.14),(1.45,0.12)],
   cones=[(1.10,0.95,1.30,8,"dark",0,0),(1.85,1.10,0.95,8,"mid",0.07,-0.04),
          (2.55,0.85,0.88,8,"dark",0,0),(3.20,0.66,0.75,7,"mid",-0.06,0.04),
          (3.80,0.55,0.44,7,"light",0,0),(4.20,0.30,0.22,6,"light",0,0)]),
 ("pine","medium"): dict(
   trunk=[(0,0.30),(0.24,0.26),(0.76,0.21),(1.34,0.18),(1.86,0.16),(2.24,0.14)],
   cones=[(1.85,1.30,1.85,9,"dark",0,0),(2.85,1.20,1.55,9,"mid",0.10,-0.06),
          (3.80,1.10,1.25,8,"dark",0,0),(4.65,0.95,0.98,8,"mid",-0.09,0.06),
          (5.40,0.80,0.72,7,"light",0,0),(6.05,0.55,0.46,7,"light",0.05,-0.03),
          (6.40,0.20,0.18,6,"light",0,0)]),
 ("pine","large"): dict(
   trunk=[(0,0.40),(0.32,0.36),(0.90,0.29),(1.60,0.25),(2.22,0.22),(2.87,0.20),(3.43,0.18)],
   cones=[(2.60,1.60,2.40,10,"dark",0,0),(3.85,1.50,2.10,10,"mid",0.12,0.07),
          (5.00,1.35,1.75,9,"dark",0,0),(6.05,1.20,1.40,9,"mid",-0.10,-0.06),
          (7.00,1.05,1.05,8,"dark",0,0),(7.85,0.85,0.72,8,"light",0.06,0.04),
          (8.55,0.55,0.44,7,"light",0,0),(8.90,0.20,0.20,6,"light",0,0)]),
 ("birch","small"): dict(
   trunk=[(0,0.16),(0.5,0.155),(1.0,0.15),(1.5,0.145),(1.98,0.14)],
   blobs=[(0.09,0.04,2.55,1.10,0.85,1.05,"bmid"),(-0.58,0.20,2.22,0.70,0.58,0.62,"bdark"),
          (0.58,-0.14,2.26,0.66,0.55,0.60,"bdark"),(0.20,0.28,2.90,0.58,0.46,0.54,"blight"),
          (-0.30,-0.30,2.85,0.52,0.42,0.48,"blight"),(0.10,-0.02,3.20,0.36,0.36,0.36,"blight")]),
 ("birch","medium"): dict(
   trunk=[(0,0.20),(0.7,0.195),(1.4,0.19),(2.1,0.185),(2.8,0.18),(3.3,0.17)],
   blobs=[(0.16,-0.06,4.05,1.55,1.05,1.45,"bmid"),(-0.78,0.24,3.50,1.00,0.78,0.92,"bdark"),
          (0.82,-0.16,3.56,0.95,0.74,0.88,"bdark"),(0.12,0.58,3.30,0.62,0.50,0.58,"bdark"),
          (0.26,0.40,4.45,0.82,0.66,0.74,"blight"),(-0.44,-0.42,4.36,0.74,0.58,0.66,"blight"),
          (0.14,-0.04,4.85,0.48,0.42,0.48,"blight")]),
 ("birch","large"): dict(
   trunk=[(0,0.26),(0.8,0.25),(1.6,0.245),(2.4,0.24),(3.2,0.235),(4.0,0.23),(4.6,0.22)],
   blobs=[(0.22,0.06,5.75,2.10,1.40,1.95,"bmid"),(-1.10,0.34,5.08,1.30,1.00,1.16,"bdark"),
          (1.14,-0.22,5.18,1.22,0.95,1.12,"bdark"),(0.18,0.88,4.74,0.78,0.62,0.72,"bdark"),
          (-0.56,-0.72,4.66,0.72,0.58,0.66,"bdark"),(0.34,0.58,6.30,1.08,0.84,0.98,"blight"),
          (-0.62,-0.58,6.18,1.00,0.80,0.92,"blight"),(0.46,-0.40,6.40,0.78,0.60,0.72,"blight"),
          (-0.26,0.50,5.95,0.74,0.58,0.68,"bdark"),(0.16,0.05,6.78,0.62,0.54,0.62,"blight")]),
 # Dead snags (species-agnostic, by size): a bark trunk tapering to a thin top
 # plus bare branches, no canopy. Mirrors the old procedural dead-tree heights +
 # branch spread (trees.rs), now with the textured bark trunk. branches:
 # (z_attach, length, base_thick, yaw, pitch).
 ("dead","small"): dict(
   trunk=[(0,0.16),(0.42,0.13),(0.86,0.11),(1.30,0.09),(1.66,0.07),(1.9,0.045)],
   branches=[(1.08,0.34,0.05,0.4,0.75),(1.34,0.30,0.045,2.6,0.82),
             (0.92,0.27,0.047,4.3,0.6),(1.6,0.24,0.04,1.4,1.05)]),
 ("dead","medium"): dict(
   trunk=[(0,0.21),(0.5,0.17),(1.05,0.15),(1.6,0.13),(2.1,0.11),(2.55,0.085),(2.85,0.05)],
   branches=[(1.7,0.46,0.065,0.5,0.7),(2.05,0.42,0.055,2.7,0.78),
             (1.35,0.38,0.06,4.4,0.55),(2.45,0.34,0.05,1.6,0.95),
             (1.95,0.30,0.045,5.6,0.85),(2.66,0.26,0.04,3.4,1.1)]),
 ("dead","large"): dict(
   trunk=[(0,0.29),(0.6,0.24),(1.25,0.21),(1.95,0.19),(2.6,0.16),(3.2,0.13),(3.7,0.10),(4.05,0.06)],
   branches=[(2.15,0.66,0.085,0.5,0.6),(2.7,0.60,0.075,2.6,0.68),
             (1.6,0.55,0.08,4.3,0.5),(3.25,0.50,0.06,1.5,0.85),
             (2.4,0.44,0.055,5.6,0.72),(3.78,0.40,0.05,3.3,1.0),
             (3.9,0.32,0.045,0.2,1.05)]),
}

cfg = CFG[(SPECIES, SIZE)]

# ---- clean scene --------------------------------------------------------------
bpy.ops.wm.read_homefile(use_empty=True)


def finish(obj, me, smooth, custom_vnormals=None):
    for p in me.polygons:
        p.use_smooth = smooth
    if me.color_attributes:
        me.color_attributes.render_color_index = me.color_attributes.find("Color")
    me.update()
    if custom_vnormals is not None:
        me.normals_split_custom_set_from_vertices(custom_vnormals)
    bpy.context.collection.objects.link(obj)


# ---- TRUNK --------------------------------------------------------------------
def canopy_top():
    tops = [bz + h for (bz, h, *_rest) in cfg.get("cones", [])]
    tops += [cz + sz for (cx, cy, cz, sx, sy, sz, _t) in cfg.get("blobs", [])]
    return max(tops) if tops else 0.0


def extended_trunk_rings():
    """The configured trunk rings, then a thin tapered spine continuing up to
    ~`TRUNK_EXTEND_FRAC` of the canopy top, so the trunk is never cut off where
    the foliage begins."""
    rings = list(cfg["trunk"])
    target = canopy_top() * TRUNK_EXTEND_FRAC
    z_last, r_last = rings[-1]
    if target > z_last + 0.3:
        n = max(1, int(math.ceil((target - z_last) / 0.6)))
        for k in range(1, n + 1):
            t = k / n
            z = z_last + (target - z_last) * t
            r = max(TRUNK_TIP_R, r_last + (TRUNK_TIP_R - r_last) * t)
            rings.append((z, r))
    return rings


def cap_trunk_ends(bm, uv, col, vr, rings):
    """Close the open top + bottom of the trunk tube with a fan to a centre
    vertex at each end, so a felled trunk reads as a solid log instead of a
    see-through pipe. `recalc_face_normals` on the now-closed manifold (called by
    the caller) orients every face outward."""
    cb = bm.verts.new((0.0, 0.0, rings[0][0]))
    ct = bm.verts.new((0.0, 0.0, rings[-1][0]))
    bm.verts.ensure_lookup_table()
    for i in range(RADIAL):
        j = (i + 1) % RADIAL
        for f, s in ((bm.faces.new((cb, vr[0][i], vr[0][j])), 0.78),
                     (bm.faces.new((ct, vr[-1][i], vr[-1][j])), 1.0)):
            for lp in f.loops:
                lp[uv].uv = (0.5, 0.5)
                lp[col] = (s, s * 0.97, s * 0.93, 1.0)


def build_trunk():
    rings = extended_trunk_rings()
    bm = bmesh.new()
    uv = bm.loops.layers.uv.new("UVMap")
    col = bm.loops.layers.float_color.new("Color")
    # vertex rings
    vr = []
    for (z, r) in rings:
        ring = []
        for i in range(RADIAL):
            a = i / RADIAL * math.tau
            ring.append(bm.verts.new((math.cos(a) * r, math.sin(a) * r, z)))
        vr.append(ring)
    bm.verts.ensure_lookup_table()
    for ri in range(len(rings) - 1):
        z0 = rings[ri][0]; z1 = rings[ri + 1][0]
        v0 = z0 / BARK_V_TILE; v1 = z1 / BARK_V_TILE
        shade0 = 0.78 if ri == 0 else 1.0   # base ring AO
        for i in range(RADIAL):
            j = (i + 1) % RADIAL
            a, b, c, d = vr[ri][i], vr[ri][j], vr[ri + 1][j], vr[ri + 1][i]
            f = bm.faces.new((a, b, c, d))
            u0 = i / RADIAL; u1 = (i + 1) / RADIAL
            uvs = [(u0, v0), (u1, v0), (u1, v1), (u0, v1)]
            shades = [shade0, shade0, 1.0, 1.0]
            for k, lp in enumerate(f.loops):
                lp[uv].uv = uvs[k]
                s = shades[k]
                lp[col] = (s, s * 0.97, s * 0.93, 1.0)
    cap_trunk_ends(bm, uv, col, vr, rings)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    me = bpy.data.meshes.new("trunk")
    bm.to_mesh(me); bm.free()
    obj = bpy.data.objects.new("trunk", me)
    finish(obj, me, smooth=True)   # real smooth round normals
    return obj


# ---- FOLIAGE: pine cones ------------------------------------------------------
def add_cone(bm, uv, col, base_z, h, R, segs, tone, xoff, yoff, vnorm):
    cx, cy = xoff, yoff
    fz = FEATHER_FZ
    z_base = base_z
    z_sh = base_z + fz * h
    r_sh = R * (1 - fz)
    apex = bm.verts.new((cx, cy, base_z + h))
    slant_full = math.hypot(h, R)
    slant_sh = math.hypot(h * (1 - fz), r_sh - 0) * 0  # placeholder
    # V measured as distance from apex along slant / texel
    v_base = slant_full / FOLIAGE_TEXEL
    v_sh = (slant_full * (1 - fz)) / FOLIAGE_TEXEL
    base_ring, sh_ring = [], []
    for i in range(segs):
        a = i / segs * math.tau
        base_ring.append(bm.verts.new((cx + math.cos(a) * R, cy + math.sin(a) * R, z_base)))
        sh_ring.append(bm.verts.new((cx + math.cos(a) * r_sh, cy + math.sin(a) * r_sh, z_sh)))
    bm.verts.ensure_lookup_table()
    circ = math.tau * R / FOLIAGE_TEXEL
    t = TONE[tone]

    def setnorm(v):
        d = Vector((v.co.x - cx, v.co.y - cy, 0.0))
        d = d.normalized() if d.length > 1e-5 else Vector((0, 0, 1))
        n = (d * (1 - UP_BIAS) + Vector((0, 0, 1)) * UP_BIAS).normalized()
        vnorm[v.index] = n
    # feather band: base_ring -> sh_ring (quads), base alpha = RIM_ALPHA
    for i in range(segs):
        j = (i + 1) % segs
        a, b, c, d = base_ring[i], base_ring[j], sh_ring[j], sh_ring[i]
        f = bm.faces.new((a, b, c, d))
        u0 = i / segs * circ; u1 = (i + 1) / segs * circ
        uvs = [(u0, v_base), (u1, v_base), (u1, v_sh), (u0, v_sh)]
        alphas = [RIM_ALPHA, RIM_ALPHA, 1.0, 1.0]
        for k, lp in enumerate(f.loops):
            lp[uv].uv = uvs[k]
            lp[col] = (t, t, t, alphas[k])
        for v in (a, b, c, d): setnorm(v)
    # upper: sh_ring -> apex (tris) with apex U = slice midpoint (no pinch)
    for i in range(segs):
        j = (i + 1) % segs
        a, b = sh_ring[i], sh_ring[j]
        f = bm.faces.new((a, b, apex))
        u0 = i / segs * circ; u1 = (i + 1) / segs * circ
        uvs = [(u0, v_sh), (u1, v_sh), ((u0 + u1) * 0.5, 0.0)]
        for k, lp in enumerate(f.loops):
            lp[uv].uv = uvs[k]
            lp[col] = (t, t, t, 1.0)
        setnorm(a); setnorm(b)
    vnorm[apex.index] = Vector((0, 0, 1))
    # Soft bottom cap: a fan from a centre vertex (opaque) to the base ring
    # (alpha 0). Fills the hollow cone interior so the canopy doesn't read as a
    # see-through shell from below / during felling, while the alpha fade to the
    # rim keeps the silhouette soft (no hard disc edge). Centre normal up-biased
    # so the underside still catches sky light (the foliage trick) instead of
    # going dark. UV reuses the side mapping so needles read at the same scale.
    cap_centre = bm.verts.new((cx, cy, z_base))
    bm.verts.ensure_lookup_table()
    for i in range(segs):
        j = (i + 1) % segs
        f = bm.faces.new((cap_centre, base_ring[j], base_ring[i]))
        u0 = i / segs * circ; u1 = (i + 1) / segs * circ
        uvs = [((u0 + u1) * 0.5, v_base * 0.5), (u1, v_base), (u0, v_base)]
        alphas = [1.0, RIM_ALPHA, RIM_ALPHA]
        for k, lp in enumerate(f.loops):
            lp[uv].uv = uvs[k]
            lp[col] = (t, t, t, alphas[k])
    vnorm[cap_centre.index] = Vector((0, 0, 1))


# ---- FOLIAGE: birch leaf blobs (octa, no underside) ---------------------------
def add_blob(bm, uv, col, cx, cy, cz, sx, sy, sz, tone, jitter, vnorm):
    top = bm.verts.new((cx, cy, cz + sz))
    bottom = bm.verts.new((cx, cy, cz - sz * 0.82))
    ring_def = [(0.95, 0.04, 0.0), (0.42, -0.05, 0.72), (-0.24, 0.12, 0.88),
                (-0.90, -0.08, 0.14), (-0.46, 0.02, -0.78), (0.38, -0.10, -0.82)]
    ring = []
    for (dx, dz, dy) in ring_def:
        ring.append(bm.verts.new((cx + sx * dx, cy + sy * dy, cz + sz * dz)))
    bm.verts.ensure_lookup_table()
    t = TONE[tone]
    tb = t * 0.82            # underside a touch darker for depth
    ju, jv = jitter

    def planar_uv(v):
        u = 0.5 + (v.co.x - cx) / (2 * sx) + ju
        w = 0.5 + (v.co.y - cy) / (2 * sy) + jv
        return (u * (2 * sx / FOLIAGE_TEXEL), w * (2 * sy / FOLIAGE_TEXEL))

    def setnorm(v):
        # Bias every vertex normal strongly toward +Z (up) so the whole leafy
        # blob lights soft from the sky, including the underside (foliage trick),
        # instead of a dark belly.
        d = Vector((v.co.x - cx, v.co.y - cy, (v.co.z - cz) * 0.5))
        d = d.normalized() if d.length > 1e-5 else Vector((0, 0, 1))
        n = (d * (1 - UP_BIAS) + Vector((0, 0, 1)) * UP_BIAS).normalized()
        vnorm[v.index] = n
    n = len(ring)
    for i in range(n):
        j = (i + 1) % n
        # Full octa: top fan + bottom fan, so the crown reads as a solid leafy
        # volume rather than a thin translucent shell.
        f = bm.faces.new((top, ring[i], ring[j]))
        for lp, vv in zip(f.loops, (top, ring[i], ring[j])):
            lp[uv].uv = planar_uv(vv)
            lp[col] = (t, t, t, 1.0)
        fb = bm.faces.new((bottom, ring[j], ring[i]))
        for lp, vv in zip(fb.loops, (bottom, ring[j], ring[i])):
            lp[uv].uv = planar_uv(vv)
            lp[col] = (tb, tb, tb, 1.0)
    setnorm(top)
    setnorm(bottom)
    for v in ring: setnorm(v)


def build_foliage():
    bm = bmesh.new()
    uv = bm.loops.layers.uv.new("UVMap")
    col = bm.loops.layers.float_color.new("Color")
    vnorm = {}
    if SPECIES == "pine":
        for (bz, h, r, segs, tone, xo, yo) in cfg["cones"]:
            add_cone(bm, uv, col, bz, h, r, segs, tone, xo, yo, vnorm)
    else:
        for bi, (cx, cy, cz, sx, sy, sz, tone) in enumerate(cfg["blobs"]):
            add_blob(bm, uv, col, cx, cy, cz, sx, sy, sz, tone,
                     (bi * 0.31 % 1.0, bi * 0.19 % 1.0), vnorm)
    me = bpy.data.meshes.new("foliage")
    bm.to_mesh(me); bm.free()
    # per-vertex up-biased normals
    normals = [vnorm.get(i, Vector((0, 0, 1))) for i in range(len(me.vertices))]
    obj = bpy.data.objects.new("foliage", me)
    finish(obj, me, smooth=True, custom_vnormals=normals)
    return obj


# ---- DEAD SNAG: bark trunk + bare branches, no canopy -------------------------
def add_dead_branch(bm, uv, col, z_attach, length, thick, yaw, pitch):
    """A tapered 4-gon stick from the trunk axis at `z_attach`, reaching out along
    (yaw, pitch). Reach matches trees.rs: +X end at
    (cos_yaw*cos_pitch, sin_pitch, -sin_yaw*cos_pitch) in game space, i.e. Blender
    (cos_yaw*cos_pitch, -sin_yaw*cos_pitch, sin_pitch)."""
    sy_, cy_ = math.sin(yaw), math.cos(yaw)
    sp, cp = math.sin(pitch), math.cos(pitch)
    reach = Vector((cy_ * cp, -sy_ * cp, sp)).normalized()
    side = reach.cross(Vector((0, 0, 1)))
    side = side.normalized() if side.length > 1e-4 else Vector((1, 0, 0))
    side2 = reach.cross(side).normalized()
    base = Vector((0, 0, z_attach))
    rings = []
    for d, th in [(0.0, thick), (length, max(0.012, thick * 0.4))]:
        c = base + reach * d
        rings.append([
            bm.verts.new(c + side * (math.cos(k / 4 * math.tau) * th)
                         + side2 * (math.sin(k / 4 * math.tau) * th))
            for k in range(4)
        ])
    bm.verts.ensure_lookup_table()
    for k in range(4):
        m = (k + 1) % 4
        f = bm.faces.new((rings[0][k], rings[0][m], rings[1][m], rings[1][k]))
        for lp in f.loops:
            lp[uv].uv = (0.5, 0.5)               # a fixed bark patch
            lp[col] = (1.0, 0.97, 0.93, 1.0)


def build_dead():
    bm = bmesh.new()
    uv = bm.loops.layers.uv.new("UVMap")
    col = bm.loops.layers.float_color.new("Color")
    rings = cfg["trunk"]                          # already tapers to a thin top
    vr = []
    for (z, r) in rings:
        vr.append([bm.verts.new((math.cos(i / RADIAL * math.tau) * r,
                                 math.sin(i / RADIAL * math.tau) * r, z))
                   for i in range(RADIAL)])
    bm.verts.ensure_lookup_table()
    for ri in range(len(rings) - 1):
        z0, z1 = rings[ri][0], rings[ri + 1][0]
        v0, v1 = z0 / BARK_V_TILE, z1 / BARK_V_TILE
        shade0 = 0.78 if ri == 0 else 1.0
        for i in range(RADIAL):
            j = (i + 1) % RADIAL
            f = bm.faces.new((vr[ri][i], vr[ri][j], vr[ri + 1][j], vr[ri + 1][i]))
            u0, u1 = i / RADIAL, (i + 1) / RADIAL
            uvs = [(u0, v0), (u1, v0), (u1, v1), (u0, v1)]
            sh = [shade0, shade0, 1.0, 1.0]
            for k, lp in enumerate(f.loops):
                lp[uv].uv = uvs[k]; s = sh[k]; lp[col] = (s, s * 0.97, s * 0.93, 1.0)
    cap_trunk_ends(bm, uv, col, vr, rings)
    for (za, length, thick, yaw, pitch) in cfg["branches"]:
        add_dead_branch(bm, uv, col, za, length, thick, yaw, pitch)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    me = bpy.data.meshes.new("trunk")
    bm.to_mesh(me); bm.free()
    obj = bpy.data.objects.new("trunk", me)
    finish(obj, me, smooth=True)
    return obj


def make_mat(name, tex, alpha):
    m = bpy.data.materials.new(name); m.use_nodes = True
    nt = m.node_tree; bsdf = nt.nodes.get("Principled BSDF")
    bsdf.inputs["Roughness"].default_value = 0.95
    bsdf.inputs["Metallic"].default_value = 0.0
    vc = nt.nodes.new("ShaderNodeVertexColor"); vc.layer_name = "Color"
    if tex and os.path.exists(tex):
        img = nt.nodes.new("ShaderNodeTexImage")
        img.image = bpy.data.images.load(tex)
        mix = nt.nodes.new("ShaderNodeMixRGB"); mix.blend_type = 'MULTIPLY'
        mix.inputs[0].default_value = 1.0
        nt.links.new(img.outputs["Color"], mix.inputs[1])
        nt.links.new(vc.outputs["Color"], mix.inputs[2])
        nt.links.new(mix.outputs["Color"], bsdf.inputs["Base Color"])
        if alpha:
            nt.links.new(img.outputs["Alpha"], bsdf.inputs["Alpha"])
            m.blend_method = 'CLIP'; m.alpha_threshold = 0.4
    else:
        nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    m.use_backface_culling = not alpha
    return m


if SPECIES == "dead":
    trunk = build_dead()
    trunk.data.materials.append(make_mat("bark", BARK_TEX, False))
    foliage = None
else:
    trunk = build_trunk()
    foliage = build_foliage()
    trunk.data.materials.append(make_mat("bark", BARK_TEX, False))
    foliage.data.materials.append(make_mat("foliage", FOLIAGE_TEX, True))

# ---- preview render -----------------------------------------------------------
if PREVIEW:
    scene = bpy.context.scene
    scene.render.engine = 'BLENDER_EEVEE'
    scene.render.film_transparent = False
    world = bpy.data.worlds.new("w"); world.use_nodes = True
    world.node_tree.nodes["Background"].inputs[0].default_value = (0.45, 0.55, 0.7, 1)
    world.node_tree.nodes["Background"].inputs[1].default_value = 0.8
    scene.world = world
    top_z = max([c[0] + c[1] for c in cfg.get("cones", [])]
                + [b[2] + b[5] for b in cfg.get("blobs", [])]
                + [r[0] for r in cfg.get("trunk", [])])
    cam_data = bpy.data.cameras.new("cam"); cam = bpy.data.objects.new("cam", cam_data)
    scene.collection.objects.link(cam); scene.camera = cam
    cam.location = (top_z * 1.1, -top_z * 1.3, top_z * 0.62)
    cam.rotation_euler = (math.radians(74), 0, math.radians(33))
    cam_data.lens = 50
    sun = bpy.data.objects.new("sun", bpy.data.lights.new("sun", 'SUN'))
    sun.data.energy = 3.5; sun.rotation_euler = (math.radians(55), 0, math.radians(40))
    scene.collection.objects.link(sun)
    scene.render.resolution_x = 640; scene.render.resolution_y = 720
    scene.render.filepath = PREVIEW
    bpy.ops.render.render(write_still=True)

# ---- export glb (trunk first, foliage second) ---------------------------------
for o in bpy.context.scene.objects:
    o.select_set(o.type == 'MESH')
bpy.context.view_layer.objects.active = trunk
bpy.ops.export_scene.gltf(
    filepath=OUT, export_format='GLB', use_selection=True, export_yup=True,
    export_apply=True, export_normals=True, export_texcoords=True,
    export_vertex_color='ACTIVE', export_materials='NONE')
print(f"EXPORTED {OUT}")
