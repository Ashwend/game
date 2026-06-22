#!/usr/bin/env python3
"""Build an Ashwend tree as a cel-shaded (toon/anime) Blender glb: a textured bark
TRUNK (round, tapered, UV-wrapped, smooth normals) + a SOLID faceted FOLIAGE
canopy (lumpy cone stack for pine / octa leaf blobs for birch, tiled UV,
UP-BIASED custom vertex normals so it lights soft like a leaf mass). The canopy
is OPAQUE solid geometry, not alpha cards: trees now join the cel family (ore
nodes + deployables), so the in-game `ToonMaterial` bands the real lighting +
draws the ink silhouette and the painted detail texture only adds grain over the
glb COLOR_0 (`texture * COLOR_0`, exactly the ore rock trick).

Two separate objects -> glb mesh0 = trunk, mesh1 = foliage. `export_materials=
'NONE'`, so Rust attaches the shared `ToonMaterial` (bark / foliage / dead-bark)
at spawn (see src/app/scene/assets.rs + .../resource_nodes/spawn.rs). The canopy
is single-sided in-game, so `build_foliage` runs `recalc_face_normals` to face
every shell outward.

Run headless:
  Blender --background --python build_tree.py -- <species> <size> <out.glb> [preview.png] [bark.png foliage.png]
species: pine|birch|dead   size: small|medium|large
Geometry constants mirror src/app/scene/mesh/trees.rs + tree_mesh_height (heights
pine 4.5/6.6/9.1, birch 3.6/5.3/7.15) for silhouette + LOD/collider parity.

------------------------------------------------------------------------------
FULL PIPELINE (ComfyUI Flux -> OpenCV -> Blender), reproducible from scratch:

1. References + measurement (proportions, not eyeballed):
   python3 art/comfy_gen.py "<anime cel tree prompt>, white background" \
       art/trees/refs/<name>.png 768 1024        # ComfyUI Flux Schnell, ~18s
   python3 art/trees/measure_tree.py art/trees/refs/<name>.png  # silhouette -> dims
   (pine_c1 = tiered conifer + visible trunk, birch_c2 = oval blob crown.)

2. Textures (toony, seamless, opaque): generated + seam-healed + contrast-softened
   straight into assets/textures/trees/ by:
   python3 art/trees/make_tree_textures.py
   -> bark_pine, bark_birch (cel bark) + foliage_pine, foliage_birch (needle/leaf
   grain). Soft + low-contrast so they ride COLOR_0 under the cel bands.

3. Models: for species in pine birch dead, size in small medium large:
   Blender --background --python art/trees/build_tree.py -- <species> <size> \
       assets/trees/<species>_<size>/model.glb "" \
       assets/textures/trees/bark_<species>.png assets/textures/trees/foliage_<species>.png
   (the `build_trees.sh` driver beside this file does all 9 at once.)

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
# Foliage normal lerp toward +Z (up). Lower than the old 0.72 so the canopy keeps
# real light/shadow FORM (top brighter, flanks darker) instead of washing out flat;
# still high enough that facets facing away from the sun don't go dark-walled.
UP_BIAS = 0.5
# Per-vertex radial jitter (fraction of radius) that breaks the smooth lathe
# cone / octa blob into a lumpy, clustered foliage mass with a ragged silhouette,
# so the canopy reads as needle/leaf clumps rather than a textured geometric solid.
CONE_JITTER = 0.26
BLOB_JITTER = 0.30
# Dead-snag trunk COLOR_0 (LINEAR): a cool, desaturated grey so the bark detail
# texture * COLOR_0 reads as weathered dead wood through the cel material, instead
# of the warm near-white the LIVE trunk uses to show full reddish/white bark.
DEAD_GREY = (0.50, 0.49, 0.50)


def hash01(a, b):
    """Deterministic pseudo-random in [0,1) from two numbers (no Math.random; the
    build must be reproducible). Classic frac(sin·k) hash."""
    return (math.sin(a * 12.9898 + b * 78.233) * 43758.5453) % 1.0
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


# ---- FOLIAGE: pine cones (SOLID, cel-shaded) ----------------------------------
def add_cone(bm, uv, col, base_z, h, R, segs, tone, xoff, yoff, vnorm):
    """A SOLID lumpy cone: jittered base ring -> apex, closed underneath with a
    fan cap. Opaque (no alpha cards); the cel `ToonMaterial` bands the lit cone +
    draws the ink silhouette, the foliage detail texture only adds needle grain
    over the COLOR_0 tone. Up-biased vertex normals keep it lighting soft like a
    leaf mass instead of facet-sparkling. `build_foliage` runs
    `recalc_face_normals` afterwards so the single-sided cull sees outward faces."""
    cx, cy = xoff, yoff
    apex = bm.verts.new((cx, cy, base_z + h))
    centre = bm.verts.new((cx, cy, base_z))       # bottom-cap hub (closes the cone)
    base_ring = []
    for i in range(segs):
        a = i / segs * math.tau
        # Per-segment radial + height jitter so the cone reads as a lumpy clustered
        # needle mass with a ragged silhouette, not a smooth lathe surface. Seeded
        # on (base_z, i) so the build stays reproducible.
        jb = 1.0 + CONE_JITTER * (hash01(base_z * 3.1, i) * 2.0 - 1.0)
        dz = (hash01(base_z * 2.3, i * 2.1) - 0.5) * h * 0.10
        base_ring.append(bm.verts.new(
            (cx + math.cos(a) * R * jb, cy + math.sin(a) * R * jb, base_z + dz)))
    bm.verts.ensure_lookup_table()
    circ = math.tau * R / FOLIAGE_TEXEL
    v_slant = math.hypot(h, R) / FOLIAGE_TEXEL
    t = TONE[tone]
    tb = t * 0.80                                  # underside a touch darker for depth

    def setnorm(v):
        d = Vector((v.co.x - cx, v.co.y - cy, 0.0))
        d = d.normalized() if d.length > 1e-5 else Vector((0, 0, 1))
        n = (d * (1 - UP_BIAS) + Vector((0, 0, 1)) * UP_BIAS).normalized()
        vnorm[v.index] = n
    # Cone sides: base_ring -> apex (tris), apex U = slice midpoint (no pinch).
    for i in range(segs):
        j = (i + 1) % segs
        f = bm.faces.new((base_ring[i], base_ring[j], apex))
        u0 = i / segs * circ; u1 = (i + 1) / segs * circ
        uvs = [(u0, v_slant), (u1, v_slant), ((u0 + u1) * 0.5, 0.0)]
        for k, lp in enumerate(f.loops):
            lp[uv].uv = uvs[k]
            lp[col] = (t, t, t, 1.0)
        setnorm(base_ring[i]); setnorm(base_ring[j])
    # Bottom cap fan: closes the cone so the canopy is a solid volume (no
    # see-through interior from below or during felling). Slightly darker tone.
    for i in range(segs):
        j = (i + 1) % segs
        f = bm.faces.new((centre, base_ring[j], base_ring[i]))
        u0 = i / segs * circ; u1 = (i + 1) / segs * circ
        uvs = [((u0 + u1) * 0.5, v_slant * 0.5), (u1, v_slant), (u0, v_slant)]
        for k, lp in enumerate(f.loops):
            lp[uv].uv = uvs[k]
            lp[col] = (tb, tb, tb, 1.0)
    vnorm[apex.index] = Vector((0, 0, 1))
    vnorm[centre.index] = Vector((0, 0, 1))


# ---- FOLIAGE: birch leaf blobs (octa, no underside) ---------------------------
def add_blob(bm, uv, col, cx, cy, cz, sx, sy, sz, tone, jitter, vnorm):
    top = bm.verts.new((cx, cy, cz + sz * (1.0 + BLOB_JITTER * (hash01(cx * 9.1, cz) - 0.5))))
    bottom = bm.verts.new((cx, cy, cz - sz * 0.82))
    ring_def = [(0.95, 0.04, 0.0), (0.42, -0.05, 0.72), (-0.24, 0.12, 0.88),
                (-0.90, -0.08, 0.14), (-0.46, 0.02, -0.78), (0.38, -0.10, -0.82)]
    ring = []
    for ri, (dx, dz, dy) in enumerate(ring_def):
        # Push each ring vertex in/out + up/down by a seeded amount so the crown is
        # a lumpy cluster of leaf clumps with a ragged silhouette, not a smooth octa
        # blob reading as a textured solid.
        jx = 1.0 + BLOB_JITTER * (hash01(cx * 7.3 + ri, cz * 4.1) * 2.0 - 1.0)
        jy = 1.0 + BLOB_JITTER * (hash01(cy * 6.7 + ri, cz * 3.3) * 2.0 - 1.0)
        jz = BLOB_JITTER * 0.5 * (hash01(cz * 5.9 + ri, cx * 2.7) * 2.0 - 1.0)
        ring.append(bm.verts.new(
            (cx + sx * dx * jx, cy + sy * dy * jy, cz + sz * (dz + jz))))
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
    # The canopy is now SOLID and single-sided (the cel `ToonMaterial` back-face
    # culls, unlike the old double-sided alpha cards). Reorient every cone/blob
    # shell so its winding faces outward, otherwise the cull hides the front and
    # the canopy renders inside-out. Custom split normals (set below, keyed by the
    # untouched vertex index) still drive the soft up-biased shading.
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
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
            lp[col] = (DEAD_GREY[0], DEAD_GREY[1], DEAD_GREY[2], 1.0)


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
                lp[uv].uv = uvs[k]; s = sh[k]
                lp[col] = (s * DEAD_GREY[0], s * DEAD_GREY[1], s * DEAD_GREY[2], 1.0)
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
    foliage.data.materials.append(make_mat("foliage", FOLIAGE_TEX, False))

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
