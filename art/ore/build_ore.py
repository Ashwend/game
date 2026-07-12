#!/usr/bin/env python3
"""Build an Ashwend ore resource-node deposit as a Blender glb: a chunky
flat-shaded faceted BOULDER studded with protruding faceted ORE CHUNKS, in the
"studded boulder" art direction (concept art/ore/concepts/form_e_v2.png, chunk
placement measured with /tmp/measure_ore.py). One glb per (ore type, depletion
stage); the boulder + chunks + rubble share a single mesh and a single material
slot, the per-mineral look rides on the COLOR_0 vertex colours (grey rock body
vs bright mineral chunks) exactly like the procedural ore + the deployables.

Mirrors `src/app/systems/items/resource_nodes/stages.rs`: 3 stages, the mound
silhouette + chunk count drop as it is mined (0 full -> 1 worn -> 2 gutted),
then the empty node despawns with a shatter (no stage-3 mesh).

Run headless:
  Blender --background --python build_ore.py -- <type> <stage> <out.glb> [preview.png] [rock_tex.png]
  type: coal|iron|sulfur|stone|ember   stage: 0|1|2
Geometry is deterministic per stage (identical silhouette across the four ore
types, like the procedural design); <type> only selects the COLOR_0 palette.
Stone vein gets no bright mineral chunks (its "chunks" are exposed darker rock),
so it stays visually distinct.

`ember` (the meteorite node) is the ONE type with a distinct
silhouette: a DARK SLAG mound (near-black, clearly darker than the shared grey
ore body) with a cluster of tall faceted CRYSTAL SPIKES erupting upward from the
crown (stage 0 full spikes, stages 1-2 progressively broken stumps, mirroring how
the ore chunk cluster depletes). The crystals carry an ember-orange COLOR_0 and,
crucially, COLOR_0 ALPHA = 1.0 as a GLOW MASK (slag alpha = 0.0); the Rust
`ToonMaterial` reads that alpha to add a night-visible emissive term only on the
crystal facets (see `docs/toon-shading.md`). Every non-meteorite type keeps alpha 1.0
throughout, and Rust binds a zero emissive tint for them, so the glow path is a
no-op for the existing ore nodes.

Z-up authoring frame, bottom at z=0; export_yup -> game +Y up, bottom at y=0 to
match the procedural meshes' anchor. export_materials='NONE' (~10 KB glbs); Rust
builds the shared StandardMaterials and supplies the rock texture as
base_color_texture.

FULL PIPELINE: Draw Things/ComfyUI concept (skill) -> OpenCV silhouette
(/tmp/measure_ore.py) -> this script -> assets.rs wiring -> headless validate.
"""
import bpy, bmesh, sys, math, os
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
TYPE = argv[0] if len(argv) > 0 else "iron"
STAGE = int(argv[1]) if len(argv) > 1 else 0
OUT = argv[2] if len(argv) > 2 else "/tmp/ore.glb"
PREVIEW = argv[3] if len(argv) > 3 else ""
ROCK_TEX = argv[4] if len(argv) > 4 else ""

TEXEL = 0.55            # metres per rock-texture tile (box projection)

# ---- COLOR_0 palette (LINEAR albedos, see docs/materials.md) ------------------
# Per user direction: the ROCK BODY is the SAME bright neutral grey on every
# node; the per-mineral identity lives entirely in the studded CHUNKS (rust =
# iron, near-black = coal, yellow = sulfur, plain rock knobs = stone vein).
# Brighter than the first pass. iron uses metallic 0.18 in Rust so its chunk
# colour part-drives F0 (rusty sheen); the others are dielectric.
_ROCK = (0.430, 0.400, 0.360)        # shared bright warm grey rock body
_ROCK_DARK = (0.280, 0.262, 0.234)   # ground-contact / crevice AO
PALETTE = {
    "iron":   dict(rock=_ROCK, rock_dark=_ROCK_DARK,
                   chunk=(0.345, 0.105, 0.038), chunk_hi=(0.580, 0.225, 0.078)),
    "coal":   dict(rock=_ROCK, rock_dark=_ROCK_DARK,
                   chunk=(0.013, 0.013, 0.018), chunk_hi=(0.115, 0.118, 0.140)),
    "sulfur": dict(rock=_ROCK, rock_dark=_ROCK_DARK,
                   chunk=(0.840, 0.560, 0.040), chunk_hi=(0.965, 0.800, 0.150)),
    "stone":  dict(rock=_ROCK, rock_dark=_ROCK_DARK,
                   chunk=(0.225, 0.210, 0.188), chunk_hi=(0.420, 0.400, 0.360)),
    # Meteorite: a DARK SLAG body (near-black basalt, well below the shared grey
    # so the node reads as scorched rock, not just a dim ore boulder) with
    # ember-orange CRYSTAL spikes. The crystal colour double-drives the emissive
    # tint in Rust, so keep it a saturated glowing orange; `_hi` lights the tips.
    "meteorite":  dict(rock=(0.055, 0.050, 0.058), rock_dark=(0.028, 0.026, 0.032),
                   chunk=(0.900, 0.320, 0.045), chunk_hi=(1.000, 0.620, 0.140)),
}
pal = PALETTE[TYPE]
# Meteorite swaps the studded-chunk cluster for erupting crystal spikes; every
# other type keeps the boulder + embedded ore-chunk build.
IS_METEORITE = (TYPE == "meteorite")

# ---- per-stage shape: boulder size + crater + chunk placements + rubble -------
# chunk placement = (azimuth_deg, elevation 0..1 (1=crown), radius). elevation
# maps to height; horizontal radius follows a circle profile so crown chunks
# cluster near the top and flank chunks sit wider. Distributed around 360 deg so
# the deposit reads as studded from every angle (the concept is one view).
# A larger, irregular (not egg-round) boulder, with the ore concentrated as a
# CLUSTER on one upper face (the type indicator) plus a couple of secondary
# chunks so the mineral still reads from other angles. The boulder shrinks and
# the cluster depletes across stages. chunk = (azimuth_deg, elevation 0..1, r).
STAGES = {
    0: dict(
        height=0.90, rscale=0.50, crater=0.0, jitter=0.22,
        chunks=[  # main cluster around az~38, upper-front face
            (38, 0.84, 0.150), (22, 0.68, 0.132), (54, 0.72, 0.130),
            (40, 0.56, 0.120), (62, 0.86, 0.108),
            # secondary indicator chunks for other-angle readability
            (172, 0.62, 0.100), (268, 0.55, 0.106)],
        rubble=[]),
    1: dict(
        height=0.68, rscale=0.46, crater=0.26, jitter=0.24,
        chunks=[(36, 0.70, 0.124), (54, 0.58, 0.112), (180, 0.52, 0.092)],
        rubble=[(48, 0.66, 0.078), (30, 0.70, 0.066), (185, 0.60, 0.060)]),
    2: dict(
        height=0.48, rscale=0.42, crater=0.50, jitter=0.26,
        chunks=[(42, 0.52, 0.104)],
        rubble=[(40, 0.70, 0.082), (58, 0.66, 0.066), (24, 0.72, 0.062),
                (200, 0.64, 0.070), (300, 0.70, 0.058)]),
}

# Meteorite stages: a squatter, darker SLAG mound topped by a cluster of tall
# faceted crystal SPIKES that erupt upward from the crown. Each spike is
# (azimuth_deg, base_elev 0..1, height, base_radius, lean_deg). Stage 0 is a full
# fan of spikes; stages 1-2 break them down to shorter stumps (the mined-out
# read), mirroring how the ore-chunk cluster depletes. A few small rubble shards
# (spilled glowing crystal) join the later stages.
METEORITE_STAGES = {
    0: dict(
        height=0.66, rscale=0.52, crater=0.0, jitter=0.20,
        spikes=[  # tall central fan, tallest in the middle, splaying outward
            (30, 0.62, 0.62, 0.075, 8),
            (0, 0.70, 0.78, 0.086, 2),
            (330, 0.60, 0.56, 0.070, -10),
            (60, 0.58, 0.50, 0.066, 16),
            (300, 0.56, 0.46, 0.062, -18),
            (150, 0.54, 0.44, 0.060, 14),
            (210, 0.55, 0.48, 0.064, -12),
            (95, 0.50, 0.36, 0.052, 24)],
        rubble=[]),
    1: dict(
        height=0.56, rscale=0.48, crater=0.20, jitter=0.22,
        spikes=[
            (10, 0.62, 0.44, 0.078, 6),
            (320, 0.56, 0.34, 0.066, -14),
            (160, 0.52, 0.30, 0.060, 12)],
        rubble=[(40, 0.62, 0.052), (330, 0.60, 0.044)]),
    2: dict(
        height=0.44, rscale=0.44, crater=0.42, jitter=0.24,
        spikes=[
            (350, 0.54, 0.26, 0.070, -6),
            (120, 0.50, 0.20, 0.058, 10)],
        rubble=[(30, 0.64, 0.050), (200, 0.60, 0.044), (280, 0.66, 0.038)]),
}
st = METEORITE_STAGES[STAGE] if IS_METEORITE else STAGES[STAGE]


def hash01(a, b):
    """Deterministic pseudo-random in [0,1) (build must be reproducible)."""
    return (math.sin(a * 12.9898 + b * 78.233) * 43758.5453) % 1.0


def box_uv(co, n):
    """Triplanar box UV: project on the plane facing the dominant normal axis so
    the stochastic rock texture tiles without polar pinch or hard stretch."""
    ax = max(range(3), key=lambda i: abs(n[i]))
    if ax == 0:
        u, w = co.y, co.z
    elif ax == 1:
        u, w = co.x, co.z
    else:
        u, w = co.x, co.y
    return (u / TEXEL, w / TEXEL)


def lerp3(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t)


def boulder_lobe(az, zc):
    """Low-frequency radial displacement giving a lumpy, irregular, craggy
    boulder silhouette (several broad bumps) instead of a smooth egg. `az` is
    the azimuth, `zc` the unit-sphere z (-1..1). Shared by the boulder build and
    the chunk seating so the cluster sits on the real (lumpy) surface."""
    return (1.0 + 0.22 * math.sin(2.0 * az + 0.7)
            + 0.15 * math.cos(3.0 * az + 1.3)
            + 0.13 * math.sin(2.4 * zc + az)
            + 0.08 * math.cos(5.0 * az + 2.1))


def paint_face(f, col, uv, color_of, alpha=1.0):
    """Flat-shade a face: per-loop COLOR_0 = (color_of(vert), alpha) and box UV.
    `alpha` is the meteorite GLOW MASK (0 = slag, 1 = crystal); every non-meteorite
    build leaves it at the default 1.0 so those glbs are byte-identical to before
    (Rust binds a zero emissive tint for them, so the alpha is inert there)."""
    n = f.normal
    for lp in f.loops:
        v = lp.vert
        lp[col] = (*color_of(v), alpha)
        lp[uv].uv = box_uv(v.co, n)


# ---- BOULDER body -------------------------------------------------------------
def add_boulder(bm, col, uv, height, rscale, crater, jitter, seed, glow_alpha=1.0):
    # subdiv 2 (320 faces) + smooth shading -> a higher-quality rounded rock
    # whose form comes from the lobes, not from coarse facets. The stylized
    # stone texture carries the surface detail.
    res = bmesh.ops.create_icosphere(bm, subdivisions=2, radius=1.0)
    vert_rad = height * 0.5
    new_faces = []
    for v in res["verts"]:
        x, y, z = v.co.x, v.co.y, v.co.z
        # Low-frequency lobe displacement gives a lumpy, irregular boulder
        # silhouette (a few broad bumps) instead of a perfectly round ball,
        # while the surface itself stays smooth-ish (low jitter + the stylized
        # texture). az/el taken on the unit sphere before scaling.
        az = math.atan2(y, x)
        lobe = boulder_lobe(az, z)
        h = hash01(x * 3.1 + seed, z * 5.7 + seed)
        f = lobe * (1.0 + jitter * (h - 0.5) * 2.0)
        v.co.x = x * rscale * f
        v.co.y = y * rscale * f
        v.co.z = z * vert_rad * f + vert_rad           # bottom ~0, top ~height
    bm.normal_update()
    # flatten the base so it sits flush on the ground; carve a mined crater into
    # the crown for the worn/gutted stages.
    for v in res["verts"]:
        if v.co.z < height * 0.10:
            v.co.z = 0.0
            v.co.x *= 1.04
            v.co.y *= 1.04                              # slight flare at the foot
        if crater > 0.0:
            rxy = math.hypot(v.co.x, v.co.y)
            top = v.co.z / max(height, 1e-4)
            # dish the upper-central verts down: strongest at the axis + crown
            dish = max(0.0, 1.0 - rxy / (rscale * 0.85)) * max(0.0, top - 0.25)
            v.co.z -= crater * height * dish
            v.co.z = max(v.co.z, 0.0)
    # identify the faces we just made (all current faces are the boulder)
    for f in bm.faces:
        new_faces.append(f)
    bmesh.ops.recalc_face_normals(bm, faces=new_faces)
    bm.normal_update()

    def color_of(v):
        hf = max(0.0, min(1.0, v.co.z / max(height, 1e-4)))
        hf = hf * hf * (3 - 2 * hf)                     # smoothstep: dark foot AO
        base = lerp3(pal["rock_dark"], pal["rock"], 0.25 + 0.75 * hf)
        n = (hash01(v.co.x * 11.0, v.co.z * 9.0) - 0.5) * 0.06   # subtle mottle
        return (max(0.0, base[0] + n), max(0.0, base[1] + n), max(0.0, base[2] + n))
    for f in new_faces:
        f.smooth = True                      # smooth-shaded rounded boulder
        # glow_alpha 0.0 on the meteorite SLAG body so only the crystals emit;
        # 1.0 (default) everywhere else (inert: Rust binds zero emissive there).
        paint_face(f, col, uv, color_of, glow_alpha)


# ---- ORE CHUNK / RUBBLE nuggets (jittered, tilted box) ------------------------
# Rough blocky nuggets, not crystals: the concept's ore reads as broken angular
# lumps embedded in the rock. 8 jittered corners + a small random tilt, 6 quad
# faces (cheap at AoI scale), upper corners take the bright mineral highlight.
_BOX_SIGNS = [(-1, -1, -1), (1, -1, -1), (1, 1, -1), (-1, 1, -1),
              (-1, -1, 1), (1, -1, 1), (1, 1, 1), (-1, 1, 1)]
_BOX_QUADS = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
              (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]


def add_chunk(bm, col, uv, center, size, seed, c_main, c_hi, tilt=0.4):
    from mathutils import Matrix
    cx, cy, cz = center
    sx, sy, sz = size
    axis = Vector((hash01(seed, 1.0) - 0.5, hash01(seed, 2.0) - 0.5,
                   hash01(seed, 3.0) - 0.5))
    axis = axis.normalized() if axis.length > 1e-4 else Vector((0, 0, 1))
    rot = Matrix.Rotation(tilt * (hash01(seed, 4.0) * 2 - 1), 4, axis)
    vs = []
    for i, (ix, iy, iz) in enumerate(_BOX_SIGNS):
        jx = 1.0 + 0.5 * (hash01(seed + 0.5, i) - 0.5)
        jy = 1.0 + 0.5 * (hash01(seed + 1.5, i) - 0.5)
        jz = 1.0 + 0.5 * (hash01(seed + 2.5, i) - 0.5)
        local = rot @ Vector((ix * sx * jx, iy * sy * jy, iz * sz * jz))
        vs.append(bm.verts.new((cx + local.x, cy + local.y, cz + local.z)))
    bm.verts.ensure_lookup_table()
    faces = [bm.faces.new([vs[k] for k in q]) for q in _BOX_QUADS]
    bmesh.ops.recalc_face_normals(bm, faces=faces)
    top_set = {vs[4], vs[5], vs[6], vs[7]}

    def color_of(v):
        t = 0.62 if v in top_set else 0.0     # bright top facets, mineral below
        return lerp3(c_main, c_hi, t)
    for f in faces:
        f.smooth = False                      # ore chunks stay crisply faceted
        paint_face(f, col, uv, color_of)


def chunk_pos(az_deg, elev, height, rscale):
    a = math.radians(az_deg)
    z = elev * height
    # circle profile, floored so crown chunks don't collapse onto the axis
    prof = max(0.22, math.sqrt(max(0.0, 1.0 - (2.0 * elev - 1.0) ** 2)))
    # match the boulder's lobe so the cluster seats on the real (lumpy) surface
    lobe = boulder_lobe(a, 2.0 * elev - 1.0)
    rxy = rscale * prof * lobe * 0.96                   # centre ~on the surface
    return (math.cos(a) * rxy, math.sin(a) * rxy, z)


# ---- EMBER CRYSTAL SPIKE (tapered faceted prism) ------------------------------
# A tall crystal: a pentagonal base ring extruded up and tapered to a point, with
# a short shoulder facet just below the tip so it reads as a cut gem rather than a
# plain cone. Leans away from the cluster axis. COLOR_0 alpha = 1.0 marks every
# crystal loop as GLOW (the slag body is 0.0), so the Rust toon material emits
# only on these facets. Faceted (flat-shaded) so the cel bands + ink edge catch
# each face like the concept's crystals.
def add_crystal(bm, col, uv, base_center, height, base_r, lean_deg, lean_az, seed,
                c_main, c_hi):
    from mathutils import Matrix
    bx, by, bz = base_center
    sides = 5
    # Lean: tilt the whole spike away from the cluster centre so the fan splays.
    lean = math.radians(lean_deg)
    lean_axis = Vector((-math.sin(lean_az), math.cos(lean_az), 0.0))
    rot = Matrix.Rotation(lean, 4, lean_axis) if abs(lean_deg) > 1e-3 else Matrix.Identity(4)
    twist = hash01(seed, 7.0) * math.tau               # random facet phase
    # base ring
    ring = []
    for i in range(sides):
        a = twist + i / sides * math.tau
        jr = base_r * (0.86 + 0.28 * hash01(seed + i, 3.0))
        local = rot @ Vector((math.cos(a) * jr, math.sin(a) * jr, 0.0))
        ring.append(bm.verts.new((bx + local.x, by + local.y, bz + local.z)))
    # shoulder ring (~80% height, pulled inward) for a gem-cut girdle
    sh = []
    sh_z = height * 0.78
    for i in range(sides):
        a = twist + i / sides * math.tau
        jr = base_r * 0.34 * (0.8 + 0.4 * hash01(seed + i, 5.0))
        local = rot @ Vector((math.cos(a) * jr, math.sin(a) * jr, sh_z))
        sh.append(bm.verts.new((bx + local.x, by + local.y, bz + local.z)))
    # apex (slightly jittered off-axis so tips aren't all dead straight)
    apex_local = rot @ Vector(((hash01(seed, 1.0) - 0.5) * base_r * 0.3,
                               (hash01(seed, 2.0) - 0.5) * base_r * 0.3, height))
    apex = bm.verts.new((bx + apex_local.x, by + apex_local.y, bz + apex_local.z))
    bm.verts.ensure_lookup_table()
    faces = []
    for i in range(sides):
        j = (i + 1) % sides
        faces.append(bm.faces.new([ring[i], ring[j], sh[j], sh[i]]))   # body facet
        faces.append(bm.faces.new([sh[i], sh[j], apex]))               # tip facet
    bmesh.ops.recalc_face_normals(bm, faces=faces)
    tip_set = set(sh) | {apex}

    def color_of(v):
        # brighter toward the tip so the crystal reads lit from within
        t = 0.7 if v in tip_set else 0.0
        return lerp3(c_main, c_hi, t)
    for f in faces:
        f.smooth = False
        paint_face(f, col, uv, color_of, 1.0)          # alpha 1.0 = GLOW mask


def crystal_base_pos(az_deg, elev, height, rscale):
    """Seat a crystal base on the slag crown, pulled toward the axis so the fan
    springs from the top of the mound rather than its flanks."""
    a = math.radians(az_deg)
    z = elev * height
    prof = max(0.20, math.sqrt(max(0.0, 1.0 - (2.0 * elev - 1.0) ** 2)))
    lobe = boulder_lobe(a, 2.0 * elev - 1.0)
    rxy = rscale * prof * lobe * 0.55                   # inward: crown cluster
    return (math.cos(a) * rxy, math.sin(a) * rxy, z)


# ---- assemble -----------------------------------------------------------------
bpy.ops.wm.read_homefile(use_empty=True)
bm = bmesh.new()
uv = bm.loops.layers.uv.new("UVMap")
col = bm.loops.layers.float_color.new("Color")

if IS_METEORITE:
    # Dark slag mound (glow_alpha 0.0 -> no emissive), then the erupting crystal
    # fan, then a few spilled glowing shards on the later stages.
    add_boulder(bm, col, uv, st["height"], st["rscale"], st["crater"], st["jitter"],
                seed=7.0, glow_alpha=0.0)
    for k, (az, elev, sh_h, base_r, lean) in enumerate(st["spikes"]):
        base = crystal_base_pos(az, elev, st["height"], st["rscale"])
        # sink the base slightly into the slag so the crystal grows out of it
        base = (base[0], base[1], base[2] - base_r * 0.4)
        add_crystal(bm, col, uv, base, sh_h, base_r, lean, math.radians(az),
                    seed=40.0 + k * 4, c_main=pal["chunk"], c_hi=pal["chunk_hi"])
    for k, (az, rad_frac, r) in enumerate(st["rubble"]):
        a = math.radians(az)
        rx = st["rscale"] * rad_frac + r
        pos = (math.cos(a) * rx, math.sin(a) * rx, r * 0.5)
        # a small leaning shard of glowing crystal (alpha 1.0 via add_crystal)
        add_crystal(bm, col, uv, pos, r * 2.4, r, 34 if k % 2 else -28,
                    a, seed=90.0 + k * 5, c_main=pal["chunk"], c_hi=pal["chunk_hi"])
else:
    add_boulder(bm, col, uv, st["height"], st["rscale"], st["crater"], st["jitter"], seed=7.0)

    for k, (az, elev, r) in enumerate(st["chunks"]):
        pos = chunk_pos(az, elev, st["height"], st["rscale"])
        # seat the nugget into the rock: pull toward the axis + drop it a touch so
        # it pokes out as an embedded lump rather than floating on the surface.
        pos = (pos[0] * 0.80, pos[1] * 0.80, pos[2] - r * 0.55)
        rr = r * (0.92 + 0.22 * hash01(k + 1, az))
        add_chunk(bm, col, uv, pos, (rr, rr * 0.9, rr * 0.85), seed=20.0 + k * 3,
                  c_main=pal["chunk"], c_hi=pal["chunk_hi"], tilt=0.5)

    for k, (az, rad_frac, r) in enumerate(st["rubble"]):
        a = math.radians(az)
        rx = st["rscale"] * rad_frac + r
        pos = (math.cos(a) * rx, math.sin(a) * rx, r * 0.4)
        # rubble: mostly broken rock, every other piece a spilled ore pebble
        is_ore = (k % 2 == 1)
        cm = pal["chunk"] if is_ore else pal["rock_dark"]
        ch = pal["chunk_hi"] if is_ore else pal["rock"]
        add_chunk(bm, col, uv, pos, (r, r * 0.85, r * 0.5), seed=80.0 + k * 5,
                  c_main=cm, c_hi=ch, tilt=0.7)

me = bpy.data.meshes.new(f"ore_{TYPE}_{STAGE}")
bm.to_mesh(me)
bm.free()
# per-face smooth flags are set in the builders (boulder smooth, chunks flat);
# bm.to_mesh preserves them, so no global override here.
if me.color_attributes:
    me.color_attributes.render_color_index = me.color_attributes.find("Color")
me.update()
obj = bpy.data.objects.new(me.name, me)
bpy.context.collection.objects.link(obj)


# ---- material (preview only; export drops it) ---------------------------------
def make_mat(name, tex):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nt = m.node_tree
    bsdf = nt.nodes.get("Principled BSDF")
    bsdf.inputs["Roughness"].default_value = 0.82 if TYPE == "iron" else 0.95
    bsdf.inputs["Metallic"].default_value = 0.18 if TYPE == "iron" else 0.0
    bsdf.inputs["Specular IOR Level"].default_value = 0.15
    vc = nt.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    if tex and os.path.exists(tex):
        img = nt.nodes.new("ShaderNodeTexImage")
        img.image = bpy.data.images.load(tex)
        mix = nt.nodes.new("ShaderNodeMixRGB")
        mix.blend_type = 'MULTIPLY'
        mix.inputs[0].default_value = 1.0
        nt.links.new(img.outputs["Color"], mix.inputs[1])
        nt.links.new(vc.outputs["Color"], mix.inputs[2])
        nt.links.new(mix.outputs["Color"], bsdf.inputs["Base Color"])
    else:
        nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    m.use_backface_culling = True
    return m


obj.data.materials.append(make_mat(f"ore_{TYPE}", ROCK_TEX))

# ---- preview render -----------------------------------------------------------
if PREVIEW:
    scene = bpy.context.scene
    scene.render.engine = 'BLENDER_EEVEE'
    world = bpy.data.worlds.new("w")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs[0].default_value = (0.5, 0.55, 0.62, 1)
    world.node_tree.nodes["Background"].inputs[1].default_value = 0.9
    scene.world = world
    cam_data = bpy.data.cameras.new("cam")
    cam = bpy.data.objects.new("cam", cam_data)
    scene.collection.objects.link(cam)
    scene.camera = cam
    d = max(st["height"], st["rscale"] * 2) * 1.9
    cam.location = (d * 0.95, -d * 1.15, st["height"] * 0.58 + 0.18)
    cam.rotation_euler = (math.radians(80), 0, math.radians(40))
    cam_data.lens = 52
    sun = bpy.data.objects.new("sun", bpy.data.lights.new("sun", 'SUN'))
    sun.data.energy = 3.2
    sun.rotation_euler = (math.radians(52), 0, math.radians(35))
    scene.collection.objects.link(sun)
    scene.render.resolution_x = 480
    scene.render.resolution_y = 480
    scene.render.filepath = PREVIEW
    bpy.ops.render.render(write_still=True)

# ---- export glb (single mesh, primitive 0) -----------------------------------
for o in bpy.context.scene.objects:
    o.select_set(o.type == 'MESH')
bpy.context.view_layer.objects.active = obj
os.makedirs(os.path.dirname(OUT), exist_ok=True)
bpy.ops.export_scene.gltf(
    filepath=OUT, export_format='GLB', use_selection=True, export_yup=True,
    export_apply=True, export_normals=True, export_texcoords=True,
    export_vertex_color='ACTIVE', export_materials='NONE')
print(f"EXPORTED {OUT}")
