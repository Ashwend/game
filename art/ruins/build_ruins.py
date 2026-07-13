"""Parametric burnt-out houses for the ruin POIs, plus the salvage chest.

The world's ruins are homes gutted by the ongoing meteor storm: charred plank
walls burnt down to jagged stubs, a scorched stone floor plinth, fallen
rafters, rubble, and (on the larger homes) a lone standing chimney. Four shell
glbs, one per `RuinPrefab` (`src/world/ruins.rs`), each exported as a SINGLE
mesh with TWO material slots so the game gets two primitives:

  primitive 0 "timber"  - charred planks, posts, rafters, floor debris.
                          Bound to `toon_wood_material` (plank line-art
                          multiplied by the near-black COLOR_0).
  primitive 1 "masonry" - stone plinth, rubble piles, chimney.
                          Bound to `toon_stone_material`.

Also built here: `ruin_cache_chest.glb`, the small charred-wood, iron-banded
salvage chest spawned on the plinth (single timber primitive, ~0.92 x 0.60 x
0.66 m to match the registry collider).

LAYOUT CONTRACT: the plinth extents, wall segment spans/heights, door gaps,
and chimney footprints MIRROR the collider tables in `src/world/ruins.rs`
(`COTTAGE_WALLS` etc.). Edit the two together. Wall plank heights are drawn
around the collider height (envelope +-25%), so the AABBs stay honest.

Z-FIGHT RULE: nothing here shares a plane. Every plank gets its own depth /
tilt / height jitter, rafters and floor boards are tilted off the plinth,
rubble and posts sink INTO the plinth instead of resting flush on it. The
shells replace the old building-piece kitbash whose abutting foundation faces
z-fought.

Deterministic build (hash01, no random module), Blender Z-up authoring;
export_yup=True flips to the game's +Y up. Origin at the site centre, ground
at z = 0, plinth top at FLOOR_TOP (0.4 m, the building-foundation height).
Blender (x, y) here is the game's plan (x, z); coordinates in the tables
below carry over 1:1 from the Rust collider tables.

Run headless (also renders a preview PNG per asset into art/ruins/preview/):
  /Applications/Blender.app/Contents/MacOS/Blender --background \
      --python art/ruins/build_ruins.py
"""

import bpy
import bmesh
import math
import os

from mathutils import Matrix, Vector

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT_DIR = os.path.join(REPO, "assets", "ruins")
PREVIEW_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "preview")

# Texture repeat (metres per tile), matching the deployable wood/stone tiles.
TILE = 0.95

# Plinth top height. MUST equal FLOOR_TOP_M (FOUNDATION_HEIGHT_M) in Rust.
FLOOR_TOP = 0.4

# ---- COLOR_0 palette (LINEAR albedo, multiplied by the toon detail textures).
CHAR = (0.032, 0.028, 0.026)        # deep charcoal plank body
CHAR_BROWN = (0.075, 0.047, 0.028)  # scorched brown grain low on a plank
CHAR_TIP = (0.100, 0.096, 0.092)    # ash-grey burnt tip
POST = (0.024, 0.021, 0.019)        # corner posts, burnt hardest
BEAM = (0.040, 0.033, 0.028)        # fallen rafters
STONE = (0.300, 0.285, 0.262)       # plinth stone
STONE_DK = (0.165, 0.155, 0.142)    # plinth sides / rubble shadow
SOOT = (0.085, 0.080, 0.076)        # soot staining near walls + chimney top
IRON = (0.030, 0.028, 0.030)        # chest banding
IRON_LT = (0.062, 0.058, 0.060)     # lit iron edge
CHEST_WOOD = (0.058, 0.040, 0.028)  # chest body, charred but still wood
CHEST_LID = (0.078, 0.062, 0.048)   # ash-dusted lid
LATCH = (0.240, 0.160, 0.070)       # dull brass latch plate (the eye-catcher)


def hash01(a, b):
    """Deterministic pseudo-random in [0,1) (build must be reproducible)."""
    return (math.sin(a * 12.9898 + b * 78.233) * 43758.5453) % 1.0


def lerp3(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t)


def cube_uv(co, n, tile):
    """Per-face box projection in Blender Z-up space: project on the plane
    facing the dominant normal axis so the plank/cobble line-art maps without
    stretch."""
    ax = max(range(3), key=lambda i: abs(n[i]))
    if ax == 0:
        u, w = co.y, co.z
    elif ax == 1:
        u, w = co.x, co.z
    else:
        u, w = co.x, co.y
    return (u / tile, w / tile)


class Shell:
    """One asset under construction: a single bmesh with the timber (slot 0)
    and masonry (slot 1) face sets."""

    TIMBER = 0
    MASONRY = 1

    def __init__(self, name):
        self.name = name
        self.bm = bmesh.new()
        self.uv = self.bm.loops.layers.uv.new("UVMap")
        self.col = self.bm.loops.layers.float_color.new("Color")

    def paint(self, faces, slot, color_of):
        for f in faces:
            f.material_index = slot
            f.smooth = False
            n = f.normal
            for lp in f.loops:
                lp[self.col] = (*color_of(lp.vert), 1.0)
                lp[self.uv].uv = cube_uv(lp.vert.co, n, TILE)

    def add_box(self, center, half, slot, color_of, rot=None, sink=0.0):
        """A box at `center` (plan x, plan z, height) with `half` extents;
        `rot` is an optional Matrix; `sink` drops it on z so it embeds instead
        of sitting flush.

        GAME-SPACE CONTRACT: every vertex negates its Blender-Y here. The
        glTF Y-up export maps Blender +Y to game -Z, so authoring plan-Z
        straight onto Blender +Y ships a Z-MIRRORED shell whose door gap
        renders on the opposite side of its collider (an invisible wall in
        the visible doorway). Negating at this single choke point keeps every
        table in this file in the same plan space as the Rust collider
        tables. Face winding flips with the mirror; the per-box
        `recalc_face_normals` below runs on the final mirrored geometry, so
        normals stay outward."""
        cx, cy, cz = center
        hx, hy, hz = half
        rot = rot or Matrix.Identity(4)
        vs = []
        for sx, sy, sz in ((-1, -1, -1), (1, -1, -1), (1, 1, -1), (-1, 1, -1),
                           (-1, -1, 1), (1, -1, 1), (1, 1, 1), (-1, 1, 1)):
            local = rot @ Vector((sx * hx, sy * hy, sz * hz))
            vs.append(self.bm.verts.new(
                (cx + local.x, -(cy + local.y), cz + local.z - sink)))
        faces = []
        for q in ((0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
                  (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)):
            faces.append(self.bm.faces.new([vs[k] for k in q]))
        bmesh.ops.recalc_face_normals(self.bm, faces=faces)
        self.paint(faces, slot, color_of)
        return vs, faces

    def finish(self, timber_only=False):
        me = bpy.data.meshes.new(self.name)
        self.bm.to_mesh(me)
        self.bm.free()
        if me.color_attributes:
            me.color_attributes.render_color_index = me.color_attributes.find("Color")
        me.update()
        obj = bpy.data.objects.new(self.name, me)
        bpy.context.collection.objects.link(obj)
        obj.data.materials.append(make_mat("timber"))
        if not timber_only:
            obj.data.materials.append(make_mat("masonry"))
        return obj


def make_mat(name):
    """Vertex-colour preview material; the game ignores it and binds its own
    toon materials, but the SLOT ORDER (timber 0, masonry 1) defines the glb
    primitive order the Rust loader relies on."""
    m = bpy.data.materials.get(name)
    if m:
        return m
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nt = m.node_tree
    bsdf = nt.nodes.get("Principled BSDF")
    bsdf.inputs["Roughness"].default_value = 0.95
    vc = nt.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    m.use_backface_culling = True
    return m


# ---------------------------------------------------------------- timber bits
def plank_env(profile, t):
    """Surviving-height envelope along a wall (fraction of the wall height).
    DETERMINISTIC direction: a "slope" wall always descends toward the wall's
    +axis end, because the Rust collider tables split sloped walls into a
    tall and a low box on exactly that assumption (no random flip)."""
    if profile == "tall":
        return 0.92 - 0.18 * abs(math.sin(t * 9.0))
    if profile == "slope":
        return 1.05 - 0.60 * t
    if profile == "gable":
        return 0.45 + 0.55 * (1.0 - abs(2.0 * t - 1.0))
    # "stub": a low, even burn line.
    return 0.95 - 0.20 * abs(math.sin(t * 7.0))


def add_plank_wall(shell, axis, c, hl, h, profile, seed):
    """A run of charred vertical planks along `axis` ('x' or 'z') centred at
    `c = (cx, cz)`, half-length `hl`, wall height `h` (the collider height).
    Every plank gets its own height / depth / tilt jitter, and a few planks
    are burnt away entirely, so the silhouette reads gutted and no two faces
    are coplanar."""
    cx, cz = c
    span = hl * 2.0
    n = max(4, int(round(span / 0.31)))
    w = span / n
    for i in range(n):
        t = (i + 0.5) / n
        # Burn-through gaps, never at the corners (the collider stays a solid
        # run; a knee-high gap in the visual reads as burnt, not passable).
        if 0 < i < n - 1 and hash01(seed + i, 3.1) < 0.10:
            continue
        env = plank_env(profile, t)
        ph = max(0.28, h * env * (0.82 + 0.36 * hash01(seed + i, 5.7)))
        ph = min(ph, h * 1.12)
        along = -hl + (i + 0.5) * w
        # Per-plank jitter: depth offset, plank half sizes, tiny yaw + lean.
        d_off = (hash01(seed + i, 9.2) - 0.5) * 0.045
        pw = w * (0.44 + 0.10 * hash01(seed + i, 11.4))
        pt = 0.055 + 0.02 * hash01(seed + i, 13.6)
        yaw = (hash01(seed + i, 15.8) - 0.5) * 0.06
        lean = (hash01(seed + i, 19.1) - 0.5) * 0.07
        if axis == "x":
            px, py = cx + along, cz + d_off
            rot = Matrix.Rotation(yaw, 4, "Z") @ Matrix.Rotation(lean, 4, "X")
            half = (pw, pt, ph / 2.0)
        else:
            px, py = cx + d_off, cz + along
            rot = Matrix.Rotation(yaw, 4, "Z") @ Matrix.Rotation(lean, 4, "Y")
            half = (pt, pw, ph / 2.0)
        base = FLOOR_TOP - 0.02  # sunk into the plinth, never flush
        center = (px, py, base + ph / 2.0)
        body = 0.55 + 0.45 * hash01(seed + i, 23.5)

        def color_of(v, ph=ph, base=base, body=body):
            zf = max(0.0, min(1.0, (v.co.z - base) / max(ph, 1e-4)))
            c0 = lerp3(CHAR_BROWN, CHAR, body)
            if zf > 0.82:
                return lerp3(c0, CHAR_TIP, (zf - 0.82) / 0.18)
            return lerp3(c0, CHAR, zf * 0.5)

        vs, _faces = shell.add_box(center, half, Shell.TIMBER, color_of, rot=rot)
        # Slant the charred top: pull each top corner down a different amount.
        for k, v in enumerate(vs[4:8]):
            v.co.z -= ph * 0.16 * hash01(seed + i, 31.0 + k)
    shell.bm.normal_update()


def add_corner_post(shell, x, z, h, seed):
    """A burnt square stud at a wall corner, a touch taller than its planks."""
    ph = h * (0.95 + 0.25 * hash01(seed, 41.0))
    yaw = (hash01(seed, 43.0) - 0.5) * 0.08
    rot = Matrix.Rotation(yaw, 4, "Z")
    base = FLOOR_TOP - 0.03

    def color_of(v, ph=ph, base=base):
        zf = max(0.0, min(1.0, (v.co.z - base) / max(ph, 1e-4)))
        return lerp3(POST, CHAR_TIP, max(0.0, zf - 0.75) * 2.0)

    vs, _ = shell.add_box((x, z, base + ph / 2.0), (0.085, 0.085, ph / 2.0),
                          Shell.TIMBER, color_of, rot=rot)
    for k, v in enumerate(vs[4:8]):
        v.co.z -= ph * 0.10 * hash01(seed, 47.0 + k)
    shell.bm.normal_update()


def add_beam(shell, p0, p1, half_w, half_t, seed):
    """A fallen charred rafter from `p0` to `p1` (Blender-space points, plan
    x/y + height z). An oriented box, so both ends sit off any flat plane."""
    a = Vector(p0)
    b = Vector(p1)
    mid = (a + b) / 2.0
    d = b - a
    length = d.length
    rot = d.to_track_quat("X", "Z").to_matrix().to_4x4()
    roll = Matrix.Rotation((hash01(seed, 53.0) - 0.5) * 0.9, 4, "X")

    def color_of(v):
        m = 0.5 + 0.5 * hash01(v.co.x * 7.1, v.co.y * 5.3)
        return lerp3(BEAM, CHAR, m)

    shell.add_box(tuple(mid), (length / 2.0, half_w, half_t), Shell.TIMBER,
                  color_of, rot=rot @ roll)


def support_height(plinths, x, z):
    """The resting height for debris at plan position `(x, z)`: the plinth
    top when a plinth slab is underneath, the bare ground otherwise. This is
    what keeps every scattered chunk grounded; without it, debris spilling
    past the plinth edge floats 0.4 m over the terrain."""
    for (cx, cz), (hx, hz) in plinths:
        if abs(x - cx) <= hx and abs(z - cz) <= hz:
            return FLOOR_TOP
    return 0.0


def add_floor_boards(shell, plinths, spots, seed):
    """A few loose scorched boards lying on the floor, tilted a hair so they
    never share the floor plane. Rest height follows what is actually
    underneath (plinth or ground)."""
    for k, (x, z) in enumerate(spots):
        yaw = hash01(seed + k, 61.0) * math.tau
        tilt = 0.03 + 0.05 * hash01(seed + k, 67.0)
        rot = Matrix.Rotation(yaw, 4, "Z") @ Matrix.Rotation(tilt, 4, "Y")
        ln = 0.5 + 0.5 * hash01(seed + k, 71.0)

        def color_of(v):
            m = hash01(v.co.x * 9.7, v.co.y * 8.1)
            return lerp3(CHAR, CHAR_BROWN, m * 0.6)

        base = support_height(plinths, x, z)
        shell.add_box((x, z, base + 0.045), (ln / 2.0, 0.075, 0.018),
                      Shell.TIMBER, color_of, rot=rot)


# --------------------------------------------------------------- masonry bits
def add_plinth(shell, c, half, walls, seed):
    """The scorched stone floor slab: ground to FLOOR_TOP, top rim jittered so
    abutting geometry never lands coplanar. Soot-stained near the wall lines."""
    cx, cz = c
    hx, hz = half

    def near_wall(x, y):
        d = 1e9
        for wl in walls:
            wx, wz = wl["c"]
            if wl["axis"] == "x":
                dd = max(abs(x - wx) - wl["hl"], 0.0) + abs(y - wz)
            else:
                dd = max(abs(y - wz) - wl["hl"], 0.0) + abs(x - wx)
            d = min(d, dd)
        return d

    def color_of(v):
        if v.co.z < FLOOR_TOP * 0.6:
            return STONE_DK
        # Vertex coords are game-space (Y mirrored by add_box); the wall
        # tables are plan-space, so un-mirror for the soot lookup.
        soot = max(0.0, 1.0 - near_wall(v.co.x, -v.co.y) / 0.9)
        mottle = (hash01(v.co.x * 6.3, v.co.y * 7.9) - 0.5) * 0.05
        base = lerp3(STONE, SOOT, soot * 0.55)
        return (max(0.0, base[0] + mottle), max(0.0, base[1] + mottle),
                max(0.0, base[2] + mottle))

    vs, _ = shell.add_box((cx, cz, FLOOR_TOP / 2.0), (hx, hz, FLOOR_TOP / 2.0),
                          Shell.MASONRY, color_of)
    # Jitter the top rim in/out a touch: a hand-laid, fire-cracked slab edge.
    for k, v in enumerate(vs[4:8]):
        v.co.x += (hash01(seed, 83.0 + k) - 0.5) * 0.05
        v.co.y += (hash01(seed, 89.0 + k) - 0.5) * 0.05
    shell.bm.normal_update()


def add_chimney(shell, x, z, h, seed):
    """A lone standing chimney: a column of jittered stone courses rising off
    the plinth, soot-blackened toward the flue."""
    courses = max(4, int(h / 0.42))
    ch = h / courses
    for i in range(courses):
        fp = 0.27 + (hash01(seed + i, 97.0) - 0.5) * 0.03
        ox = (hash01(seed + i, 101.0) - 0.5) * 0.035
        oz = (hash01(seed + i, 103.0) - 0.5) * 0.035
        yaw = (hash01(seed + i, 107.0) - 0.5) * 0.10
        rot = Matrix.Rotation(yaw, 4, "Z")
        zf = i / max(courses - 1, 1)

        def color_of(v, zf=zf):
            mottle = (hash01(v.co.x * 8.8, v.co.z * 6.1) - 0.5) * 0.05
            base = lerp3(STONE, SOOT, 0.25 + 0.6 * zf)
            return (max(0.0, base[0] + mottle), max(0.0, base[1] + mottle),
                    max(0.0, base[2] + mottle))

        vs, _ = shell.add_box(
            (x + ox, z + oz, FLOOR_TOP - 0.04 + (i + 0.5) * ch),
            (fp, fp, ch / 2.0 + 0.01), Shell.MASONRY, color_of, rot=rot)
        if i == courses - 1:  # cracked top course
            for k, v in enumerate(vs[4:8]):
                v.co.z -= ch * 0.5 * hash01(seed + i, 109.0 + k)
    shell.bm.normal_update()


def add_rubble(shell, plinths, x, z, seed, count=6, spread=0.5):
    """A pile of fire-cracked wall stone. Each chunk rests on whatever is
    actually beneath it (plinth top or bare ground), so a pile at the slab
    edge spills DOWN off it instead of hovering in the air. Piles are placed
    clear of the door/cart openings so nothing litters the way in."""
    for k in range(count):
        a = hash01(seed + k, 113.0) * math.tau
        r = spread * math.sqrt(hash01(seed + k, 127.0))
        s = 0.07 + 0.10 * hash01(seed + k, 131.0)
        yaw = hash01(seed + k, 137.0) * math.tau
        rot = (Matrix.Rotation(yaw, 4, "Z")
               @ Matrix.Rotation((hash01(seed + k, 139.0) - 0.5) * 0.7, 4, "X"))
        dark = 0.3 + 0.6 * hash01(seed + k, 149.0)

        def color_of(v, dark=dark):
            return lerp3(STONE, STONE_DK, dark)

        px = x + math.cos(a) * r
        pz = z + math.sin(a) * r
        base = support_height(plinths, px, pz)
        shell.add_box((px, pz, base + s * 0.35),
                      (s, s * 0.85, s * 0.6), Shell.MASONRY, color_of,
                      rot=rot, sink=0.05)


# ------------------------------------------------------------------ the homes
# Wall tables MIRROR src/world/ruins.rs (COTTAGE_WALLS etc.): axis, centre,
# half-length, collider height, plus the visual-only burn profile. Debris
# spots are placed by hand to stay clear of the chest spawn points
# (RuinPrefab::cache_points).
HOUSES = {
    "burnt_cottage": dict(
        plinths=[((0.0, 0.0), (3.2, 2.45))],
        walls=[
            dict(axis="x", c=(0.0, -2.1), hl=2.85, h=2.2, profile="tall"),
            dict(axis="z", c=(-2.85, 0.0), hl=1.95, h=1.8, profile="slope"),
            dict(axis="z", c=(2.85, 0.0), hl=1.95, h=0.8, profile="stub"),
            dict(axis="x", c=(-1.425, 2.1), hl=1.425, h=1.0, profile="stub"),
            dict(axis="x", c=(2.025, 2.1), hl=0.825, h=1.4, profile="slope"),
        ],
        posts=[(-2.85, -2.1, 2.0), (2.85, -2.1, 1.6), (-2.85, 2.1, 1.1),
               (2.85, 2.1, 1.3)],
        chimney=(1.9, -2.1, 2.6),
        beams=[((-0.4, -1.9, 2.2), (-1.6, 0.9, 0.46)),
               ((1.2, -1.8, 1.9), (2.2, 0.6, 0.46))],
        boards=[(-1.6, -0.4), (0.2, 0.8), (-0.4, 1.5)],
        # Clear of the door gap (x 0.0..1.2 at z 2.1).
        rubble=[(2.9, 0.6), (-2.4, 1.9)],
    ),
    "burnt_farmhouse": dict(
        plinths=[((-0.75, 0.0), (3.75, 2.75)), ((4.5, 1.0), (1.5, 1.75))],
        walls=[
            dict(axis="x", c=(-0.75, -2.55), hl=3.55, h=2.3, profile="tall"),
            dict(axis="z", c=(-4.3, 0.0), hl=2.4, h=1.6, profile="slope"),
            dict(axis="x", c=(-2.975, 2.55), hl=1.325, h=0.9, profile="stub"),
            dict(axis="x", c=(1.175, 2.55), hl=1.625, h=1.2, profile="slope"),
            dict(axis="z", c=(5.8, 1.0), hl=1.55, h=1.4, profile="slope"),
            dict(axis="x", c=(4.4, 2.55), hl=1.4, h=1.0, profile="stub"),
            dict(axis="x", c=(4.4, -0.55), hl=1.4, h=0.6, profile="stub"),
        ],
        posts=[(-4.3, -2.55, 2.1), (2.8, -2.55, 1.9), (-4.3, 2.55, 1.2),
               (2.8, 2.55, 1.3), (5.8, -0.55, 1.0), (5.8, 2.55, 1.2)],
        chimney=(-4.3, -0.9, 3.0),
        beams=[((-2.4, -2.3, 2.3), (-0.4, -0.4, 0.46)),
               ((1.5, -2.35, 2.0), (0.4, 0.9, 0.46)),
               ((5.6, 0.4, 1.5), (3.6, 1.7, 0.46))],
        boards=[(-1.8, 0.6), (0.6, -1.2), (4.0, 0.2), (-3.2, 1.4)],
        # Clear of the door gap (x -1.65..-0.45 at z 2.55).
        rubble=[(-3.9, 2.35), (2.9, -2.5), (4.4, 2.6)],
    ),
    "burnt_shed": dict(
        plinths=[((0.0, 0.0), (1.95, 1.65))],
        walls=[
            dict(axis="x", c=(0.0, -1.4), hl=1.7, h=1.6, profile="slope"),
            dict(axis="z", c=(-1.7, 0.0), hl=1.35, h=1.2, profile="slope"),
            dict(axis="z", c=(1.7, 0.0), hl=1.35, h=0.7, profile="stub"),
        ],
        posts=[(-1.7, -1.4, 1.5), (1.7, -1.4, 1.2), (-1.7, 1.4, 0.9),
               (1.7, 1.4, 0.7)],
        chimney=None,
        beams=[((-0.9, -1.2, 1.5), (0.9, 0.9, 0.46))],
        boards=[(0.6, 0.3)],
        rubble=[(1.55, 0.9)],
    ),
    "burnt_barn": dict(
        plinths=[((0.0, 0.0), (4.2, 2.95))],
        walls=[
            dict(axis="z", c=(-3.9, 0.0), hl=2.6, h=2.8, profile="gable"),
            dict(axis="z", c=(3.9, 0.0), hl=2.6, h=2.0, profile="gable"),
            dict(axis="x", c=(-2.55, -2.6), hl=1.35, h=1.0, profile="stub"),
            dict(axis="x", c=(2.55, -2.6), hl=1.35, h=1.0, profile="stub"),
            dict(axis="x", c=(0.0, 2.6), hl=3.75, h=0.7, profile="stub"),
        ],
        posts=[(-3.9, -2.6, 1.8), (-3.9, 2.6, 1.8), (3.9, -2.6, 1.5),
               (3.9, 2.6, 1.5)],
        chimney=None,
        beams=[((-3.6, -0.8, 2.4), (-1.2, -1.6, 0.46)),
               ((-3.6, 1.0, 2.2), (-1.6, 1.9, 0.46)),
               ((3.6, 0.4, 1.7), (1.8, -0.9, 0.46))],
        boards=[(-0.8, 0.6), (1.4, -1.2), (0.2, 1.8)],
        # Clear of the cart opening (x -1.2..1.2 at z -2.6).
        rubble=[(3.2, -2.5), (2.6, 2.6)],
    ),
}


def build_house(name, spec, seed):
    shell = Shell(name)
    for k, (c, half) in enumerate(spec["plinths"]):
        add_plinth(shell, c, half, spec["walls"], seed + k * 7.7)
    for k, wl in enumerate(spec["walls"]):
        add_plank_wall(shell, wl["axis"], wl["c"], wl["hl"], wl["h"],
                       wl["profile"], seed + 100.0 + k * 13.3)
    for k, (px, pz, ph) in enumerate(spec["posts"]):
        add_corner_post(shell, px, pz, ph, seed + 200.0 + k * 3.1)
    if spec["chimney"]:
        cxx, czz, chh = spec["chimney"]
        add_chimney(shell, cxx, czz, chh, seed + 300.0)
    for k, (p0, p1) in enumerate(spec["beams"]):
        add_beam(shell, p0, p1, 0.07, 0.09, seed + 400.0 + k * 5.9)
    add_floor_boards(shell, spec["plinths"], spec["boards"], seed + 500.0)
    for k, (rx, rz) in enumerate(spec["rubble"]):
        add_rubble(shell, spec["plinths"], rx, rz, seed + 600.0 + k * 11.1)
    return shell.finish()


# ------------------------------------------------------------- salvage chest
def build_chest():
    """The salvage chest: a small charred-wood box with near-black iron
    bands and a dull brass latch. ~0.92 x 0.60 x 0.66 m, matching the
    registry collider (half 0.46 / 0.33). Single timber primitive."""
    shell = Shell("ruin_cache_chest")
    body_h = 0.46
    lid_h = 0.17
    hw, hd = 0.44, 0.28

    def wood(v):
        zf = max(0.0, min(1.0, v.co.z / (body_h + lid_h)))
        m = (hash01(v.co.x * 11.0, v.co.z * 9.0) - 0.5) * 0.02
        base = lerp3(CHEST_WOOD, CHEST_LID, zf * 0.5)
        return (max(0.0, base[0] + m), max(0.0, base[1] + m), max(0.0, base[2] + m))

    def lid(v):
        m = (hash01(v.co.x * 9.0, v.co.y * 7.0) - 0.5) * 0.02
        return (max(0.0, CHEST_LID[0] + m), max(0.0, CHEST_LID[1] + m),
                max(0.0, CHEST_LID[2] + m))

    def iron(v):
        lit = 1.0 if v.co.z > body_h * 0.9 else 0.0
        return lerp3(IRON, IRON_LT, lit * 0.7)

    def brass(_v):
        return LATCH

    # Body, slightly tapered by pulling the base inward.
    vs, _ = shell.add_box((0.0, 0.0, body_h / 2.0), (hw, hd, body_h / 2.0),
                          Shell.TIMBER, wood)
    for v in vs[0:4]:
        v.co.x *= 0.94
        v.co.y *= 0.94
    # Lid: a shallower, slightly overhanging box with a cambered top.
    vs, _ = shell.add_box((0.0, 0.0, body_h + lid_h / 2.0),
                          (hw + 0.02, hd + 0.02, lid_h / 2.0),
                          Shell.TIMBER, lid)
    for v in vs[4:8]:
        v.co.x *= 0.90
        v.co.y *= 0.82
    # Two iron straps wrapping body + lid (offset out so nothing is coplanar).
    for sx in (-hw * 0.52, hw * 0.52):
        shell.add_box((sx, 0.0, (body_h + lid_h) / 2.0),
                      (0.045, hd + 0.036, (body_h + lid_h) / 2.0 + 0.012),
                      Shell.TIMBER, iron)
    # Brass latch plate + iron hasp on the front face.
    shell.add_box((0.0, hd + 0.028, body_h - 0.02), (0.055, 0.016, 0.075),
                  Shell.TIMBER, brass)
    shell.add_box((0.0, hd + 0.044, body_h + 0.035), (0.032, 0.014, 0.045),
                  Shell.TIMBER, iron)
    shell.bm.normal_update()
    return shell.finish(timber_only=True)


# ----------------------------------------------------------------- export/run
def export_glb(obj, path):
    for o in bpy.context.scene.objects:
        o.select_set(o == obj)
    bpy.context.view_layer.objects.active = obj
    os.makedirs(os.path.dirname(path), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=path, export_format="GLB", use_selection=True, export_yup=True,
        export_apply=True, export_normals=True, export_texcoords=True,
        export_vertex_color="ACTIVE", export_materials="EXPORT",
        export_image_format="NONE")
    print(f"EXPORTED {path}")


def render_preview(obj, path, dist=9.0, height=4.2):
    scene = bpy.context.scene
    for o in scene.objects:
        if o.type == "MESH":
            o.hide_render = o != obj
    scene.render.engine = "BLENDER_EEVEE"
    world = bpy.data.worlds.get("w") or bpy.data.worlds.new("w")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs[0].default_value = (0.5, 0.55, 0.62, 1)
    world.node_tree.nodes["Background"].inputs[1].default_value = 0.9
    scene.world = world
    cam_data = bpy.data.cameras.get("cam") or bpy.data.cameras.new("cam")
    cam = bpy.data.objects.get("camobj") or bpy.data.objects.new("camobj", cam_data)
    if cam.name not in scene.collection.objects:
        scene.collection.objects.link(cam)
    scene.camera = cam
    cam.location = (dist * 0.8, -dist, height)
    look = Vector((0.0, 0.0, 0.9))
    direction = look - cam.location
    cam.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()
    cam_data.lens = 42
    if "sun" not in bpy.data.objects:
        sun = bpy.data.objects.new("sun", bpy.data.lights.new("sun", "SUN"))
        sun.data.energy = 3.0
        sun.rotation_euler = (math.radians(50), 0, math.radians(35))
        scene.collection.objects.link(sun)
    scene.render.resolution_x = 640
    scene.render.resolution_y = 480
    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)


def main():
    bpy.ops.wm.read_homefile(use_empty=True)
    os.makedirs(PREVIEW_DIR, exist_ok=True)
    seeds = {"burnt_cottage": 11.0, "burnt_farmhouse": 23.0,
             "burnt_shed": 37.0, "burnt_barn": 51.0}
    built = [build_house(name, spec, seeds[name]) for name, spec in HOUSES.items()]
    built.append(build_chest())
    for obj in built:
        export_glb(obj, os.path.join(OUT_DIR, f"{obj.name}.glb"))
    for obj in built:
        dist = 4.0 if obj.name == "ruin_cache_chest" else 11.0
        height = 1.6 if obj.name == "ruin_cache_chest" else 5.0
        render_preview(obj, os.path.join(PREVIEW_DIR, f"{obj.name}.png"),
                       dist=dist, height=height)


main()
