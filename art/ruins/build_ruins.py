"""Parametric ruin props for the ruin POIs.

Three cel-shaded world glbs authored the same way as the deployables and ore
nodes: box-projected UVs (so the shared toon stone line-art texture maps as
horizontal courses) plus a COLOR_0 vertex-colour identity (weathered pale stone
with dark iron bands). No new textures: Rust binds these to the existing
`DeployableVisualAssets::toon_stone_material` (the cel stone material), so the
COLOR_0 carries the stone/iron colour and the stone detail texture multiplies on
top, exactly like the furnace.

Props built (exported to assets/ruins/<name>.glb):

  * broken_pillar   - a tapered fluted stump with a fractured, angled top.
  * fallen_arch     - two short pillar stumps carrying a collapsed lintel that
                      lies across them at an angle.
  * ruin_cache_chest - a weathered stone-and-iron strongbox (the lootable), a
                      stone body banded with dark iron straps + a lid.

Blender Z-up authoring; export_yup=True flips to the game's +Y up. Origin at the
base (y = 0) so a prop sits on the ground like the other deployables.

Run headless (also renders a preview PNG per prop into art/ruins/preview/):
  /Applications/Blender.app/Contents/MacOS/Blender --background \
      --python art/ruins/build_ruins.py
"""

import bpy
import bmesh
import math
import os

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT_DIR = os.path.join(REPO, "assets", "ruins")
PREVIEW_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "preview")

# Texture repeat (metres per tile). Matches the deployable stone tile so the
# cobble line-art reads at the same scale as the furnace.
TILE = 0.95

# COLOR_0 palette (LINEAR albedo, multiplied by the near-white stone texture).
STONE = (0.480, 0.462, 0.430)      # weathered pale limestone
STONE_DK = (0.330, 0.315, 0.290)   # shadowed / ground-contact stone
STONE_LT = (0.560, 0.540, 0.505)   # sun-bleached highlight on tops
IRON = (0.110, 0.095, 0.080)       # dark rusted iron bands / straps
IRON_LT = (0.180, 0.150, 0.120)    # lit iron edge

# Cache-chest palette: pushed for read-at-distance contrast (the chest is the
# one interactable in a ruin, so it must pop against the grey foundation from
# 15-20 m). Paler stone than the masonry, near-black iron, a sun-bleached lid,
# and a warm brass trim/lock so the eye lands on it.
CHEST_STONE = (0.580, 0.560, 0.520)    # paler weathered stone body
CHEST_LID = (0.680, 0.660, 0.610)      # sun-bleached lid, lightest surface
CHEST_IRON = (0.055, 0.050, 0.045)     # near-black banding for contrast
CHEST_IRON_LT = (0.100, 0.090, 0.080)  # lit iron
CHEST_BRASS = (0.480, 0.320, 0.120)    # warm brass trim + lock plate


# ----------------------------------------------------------------- UV projection
def cube_uv(co, n, tile):
    """Per-face box projection in Blender Z-up space (matches build_deployables:
    vertical faces keep world-up as v so courses run horizontally)."""
    ax, ay, az = abs(n[0]), abs(n[1]), abs(n[2])
    if az >= ax and az >= ay:
        u, v = co[0], co[1]
    elif ax >= ay and ax >= az:
        u, v = co[1], co[2]
    else:
        u, v = co[0], co[2]
    return (u / tile, v / tile)


class Builder:
    """Accumulates faces into one bmesh with a COLOR_0 layer + a UVMap, then bakes
    a mesh. Each `add_box` paints its faces a solid colour and box-UVs them; a
    per-piece `recalc_face_normals` keeps winding correct."""

    def __init__(self):
        self.bm = bmesh.new()
        self.col = self.bm.loops.layers.color.new("Color")
        self.uv = self.bm.loops.layers.uv.new("UVMap")

    def add_box(self, center, half, color, taper_top=1.0, shear=(0.0, 0.0)):
        """A box centred at `center` with half-extents `half`. `taper_top` scales
        the top face's X/Y (a value < 1 makes a tapered stump); `shear` offsets
        the top face in X/Y (leans a lintel / fractures a top). Returns the
        created faces so the caller can recalc normals per piece."""
        cx, cy, cz = center
        hx, hy, hz = half
        tx, ty = hx * taper_top, hy * taper_top
        sx, sy = shear
        # 8 corners: bottom four then top four (top shrunk + sheared).
        b = [
            (cx - hx, cy - hy, cz - hz),
            (cx + hx, cy - hy, cz - hz),
            (cx + hx, cy + hy, cz - hz),
            (cx - hx, cy + hy, cz - hz),
        ]
        t = [
            (cx - tx + sx, cy - ty + sy, cz + hz),
            (cx + tx + sx, cy - ty + sy, cz + hz),
            (cx + tx + sx, cy + ty + sy, cz + hz),
            (cx - tx + sx, cy + ty + sy, cz + hz),
        ]
        verts = [self.bm.verts.new(p) for p in (b + t)]
        quads = [
            (0, 1, 2, 3),  # bottom
            (7, 6, 5, 4),  # top
            (0, 1, 5, 4),  # -Y
            (1, 2, 6, 5),  # +X
            (2, 3, 7, 6),  # +Y
            (3, 0, 4, 7),  # -X
        ]
        faces = []
        for a, bb, c, d in quads:
            f = self.bm.faces.new((verts[a], verts[bb], verts[c], verts[d]))
            faces.append(f)
        bmesh.ops.recalc_face_normals(self.bm, faces=faces)
        for f in faces:
            for lp in f.loops:
                lp[self.col] = (color[0], color[1], color[2], 1.0)
                lp[self.uv].uv = cube_uv(lp.vert.co, f.normal, TILE)
        return faces

    def to_object(self, name):
        me = bpy.data.meshes.new(name)
        self.bm.to_mesh(me)
        self.bm.free()
        if me.color_attributes:
            idx = me.color_attributes.find("Color")
            if idx != -1:
                me.color_attributes.render_color_index = idx
                me.color_attributes.active_color_index = idx
        me.update()
        obj = bpy.data.objects.new(name, me)
        bpy.context.collection.objects.link(obj)
        # Wire a vertex-colour material so the exporter emits COLOR_0.
        mat = bpy.data.materials.new(name + "_mat")
        mat.use_nodes = True
        nt = mat.node_tree
        bsdf = nt.nodes.get("Principled BSDF")
        vc = nt.nodes.new("ShaderNodeVertexColor")
        vc.layer_name = "Color"
        if bsdf is not None:
            nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
        obj.data.materials.append(mat)
        return obj


def reset_scene():
    bpy.ops.wm.read_homefile(use_empty=True)


def export_glb(obj, name):
    os.makedirs(OUT_DIR, exist_ok=True)
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    out = os.path.join(OUT_DIR, name + ".glb")
    bpy.ops.export_scene.gltf(
        filepath=out, export_format="GLB", use_selection=True,
        export_yup=True, export_apply=True, export_normals=True,
        export_materials="EXPORT", export_texcoords=True,
    )
    print("wrote", out)
    return out


# ------------------------------------------------------------------- prop builders
def build_broken_pillar():
    """A tapered fluted stump with a fractured top. Wide weathered base, a
    tapering fluted shaft, and a sheared/tilted broken cap."""
    b = Builder()
    # Base plinth.
    b.add_box((0, 0, 0.06), (0.34, 0.34, 0.06), STONE_DK)
    # Fluted shaft: four slim columns clustered so the silhouette reads fluted,
    # each tapering upward. Stops well short so it's clearly a broken stump.
    flute_h = 0.30
    for (ox, oy) in [(-0.14, -0.14), (0.14, -0.14), (0.14, 0.14), (-0.14, 0.14)]:
        b.add_box((ox, oy, 0.12 + flute_h), (0.09, 0.09, flute_h),
                  STONE, taper_top=0.8)
    # Core shaft filling between the flutes.
    b.add_box((0, 0, 0.12 + flute_h), (0.16, 0.16, flute_h), STONE, taper_top=0.85)
    # Fractured cap: a short block sheared to one side, sun-bleached top.
    b.add_box((0, 0, 0.12 + flute_h * 2 + 0.05), (0.15, 0.15, 0.06),
              STONE_LT, taper_top=0.7, shear=(0.10, 0.06))
    return b.to_object("broken_pillar")


def build_fallen_arch():
    """Two short pillar stumps and a collapsed lintel lying across them at an
    angle."""
    b = Builder()
    # Two stumps flanking the span.
    for ox in (-1.0, 1.0):
        b.add_box((ox, 0, 0.06), (0.30, 0.30, 0.06), STONE_DK)          # plinth
        b.add_box((ox, 0, 0.34), (0.22, 0.22, 0.22), STONE, taper_top=0.9)  # shaft
        # A cracked capital where the lintel would have rested.
        b.add_box((ox, 0, 0.58), (0.26, 0.26, 0.05), STONE_LT)
    # The collapsed lintel: a long slab lying across, tilted (leaning off one
    # capital), sheared so one end sits lower. Modelled as a long box rotated a
    # touch by shearing its top.
    lintel = b.add_box((0.0, 0.0, 0.66), (1.05, 0.24, 0.14), STONE,
                       shear=(0.12, 0.0))
    # Give the lintel a second darker under-band so the break reads.
    b.add_box((0.0, 0.0, 0.52), (0.95, 0.20, 0.05), STONE_DK)
    return b.to_object("fallen_arch")


def build_ruin_cache_chest():
    """A weathered stone-and-iron strongbox, the ruin lootable. Sized to read as
    "loot me" from 15-20 m: roughly hip height (~0.92 m) on a 1.2 x 0.88 m
    footprint, comparable to the small storage box. High-contrast identity:
    pale weathered stone body, near-black iron corner posts and wrap bands, a
    sun-bleached lighter lid, and a warm brass seam trim + lock plate."""
    b = Builder()
    # Stone base slab (darker, grounds the silhouette).
    b.add_box((0, 0, 0.07), (0.60, 0.44, 0.07), STONE_DK)
    # Pale stone body.
    b.add_box((0, 0, 0.44), (0.52, 0.38, 0.30), CHEST_STONE)
    # Near-black iron corner posts, proud of the body faces.
    for (ox, oy) in [(-0.50, -0.36), (0.50, -0.36), (0.50, 0.36), (-0.50, 0.36)]:
        b.add_box((ox, oy, 0.44), (0.07, 0.07, 0.31), CHEST_IRON)
    # Two horizontal iron bands wrapping all four faces.
    for bz in (0.26, 0.62):
        b.add_box((0, 0.39, bz), (0.52, 0.035, 0.045), CHEST_IRON_LT)   # front
        b.add_box((0, -0.39, bz), (0.52, 0.035, 0.045), CHEST_IRON_LT)  # back
        b.add_box((0.53, 0, bz), (0.035, 0.38, 0.045), CHEST_IRON_LT)   # right
        b.add_box((-0.53, 0, bz), (0.035, 0.38, 0.045), CHEST_IRON_LT)  # left
    # Warm brass seam trim where the lid meets the body (the "warm rim").
    b.add_box((0, 0, 0.745), (0.57, 0.43, 0.025), CHEST_BRASS)
    # Sun-bleached lid, overhanging the body.
    b.add_box((0, 0, 0.84), (0.56, 0.42, 0.075), CHEST_LID)
    # Dark iron lid cap band on top, so the lid edge reads at a glance.
    b.add_box((0, 0, 0.93), (0.57, 0.43, 0.02), CHEST_IRON)
    # Lock: brass plate + near-black hasp, centred on the front face.
    b.add_box((0, 0.41, 0.58), (0.11, 0.035, 0.14), CHEST_BRASS)
    b.add_box((0, 0.445, 0.58), (0.06, 0.02, 0.09), CHEST_IRON)
    return b.to_object("ruin_cache_chest")


# --------------------------------------------------------------------- preview
def render_preview(obj, name):
    """Render a quick EEVEE 3/4 preview into art/ruins/preview/<name>.png."""
    scene = bpy.context.scene
    try:
        scene.render.engine = "BLENDER_EEVEE_NEXT"
    except Exception:
        try:
            scene.render.engine = "BLENDER_EEVEE"
        except Exception:
            return
    scene.render.resolution_x = 512
    scene.render.resolution_y = 512
    scene.render.film_transparent = True
    # World light.
    world = bpy.data.worlds.new("w")
    world.use_nodes = True
    bg = world.node_tree.nodes.get("Background")
    if bg:
        bg.inputs[0].default_value = (0.55, 0.58, 0.62, 1.0)
        bg.inputs[1].default_value = 1.0
    scene.world = world
    # Sun.
    light_data = bpy.data.lights.new("sun", type="SUN")
    light_data.energy = 3.0
    light = bpy.data.objects.new("sun", light_data)
    light.rotation_euler = (math.radians(55), 0, math.radians(35))
    scene.collection.objects.link(light)
    # Camera framing a ~1.5 m tall prop from 3/4.
    cam_data = bpy.data.cameras.new("cam")
    cam = bpy.data.objects.new("cam", cam_data)
    cam.location = (2.6, -3.0, 1.9)
    cam.rotation_euler = (math.radians(66), 0, math.radians(40))
    scene.collection.objects.link(cam)
    scene.camera = cam
    os.makedirs(PREVIEW_DIR, exist_ok=True)
    scene.render.filepath = os.path.join(PREVIEW_DIR, name + ".png")
    bpy.ops.render.render(write_still=True)
    print("preview", scene.render.filepath)


def main():
    builders = [
        ("broken_pillar", build_broken_pillar),
        ("fallen_arch", build_fallen_arch),
        ("ruin_cache_chest", build_ruin_cache_chest),
    ]
    for name, fn in builders:
        reset_scene()
        obj = fn()
        export_glb(obj, name)
        render_preview(obj, name)


if __name__ == "__main__":
    main()
