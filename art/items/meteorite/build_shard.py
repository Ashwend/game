"""Build a single meteorite crystal shard glb for the pocket-item icon.

The meteorite ORE NODE (art/ore/build_ore.py) is a dark slag mound sprouting a
fan of ember-orange crystal spikes; that whole node is the wrong silhouette for a
carried material. This builds ONE faceted crystal shard, the mined form: a short,
chunky, pentagonal cut gem lying at a slight tilt, so the inventory icon reads as
"a glowing crystal you hold", the same way `wood` is a stack of logs, not a tree.

Geometry mirrors add_crystal() in build_ore.py (a pentagonal base ring, an inward
shoulder girdle at ~78% height, a jittered apex) so the shard carries the exact
faceting of the node crystals. It is proportioned squatter (a hand shard, not a
tall spike) and gets a small broken-base plinth so it does not look like it was
snapped off mid-air. COLOR_0 uses the ember palette from build_ore.py
(chunk = 0.900/0.320/0.045, chunk_hi tip = 1.000/0.620/0.140), brighter toward the
tip so it reads lit from within, matching the node.

render_icon.py bakes its own icon material from COLOR_0, so no material is needed
here beyond wiring a vertex-colour node so the exporter emits COLOR_0.

Run headless:
  /Applications/Blender.app/Contents/MacOS/Blender --background \
      --python art/items/meteorite/build_shard.py
"""

import bpy
import bmesh
import math
import os

from mathutils import Matrix, Vector

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
OUT = os.path.join(REPO, "assets", "items", "meteorite", "model.glb")

# Ember palette (LINEAR albedo), copied from art/ore/build_ore.py PALETTE["ember"].
C_MAIN = (0.900, 0.320, 0.045)   # crystal body
C_HI = (1.000, 0.620, 0.140)     # lit tip / high facet
C_DARK = (0.520, 0.170, 0.030)   # shadowed broken base


def hash01(a, b):
    return (math.sin(a * 12.9898 + b * 78.233) * 43758.5453) % 1.0


def lerp3(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def add_shard(bm, col, base_center, height, base_r, lean_deg, lean_az, seed):
    """A single tapered pentagonal cut crystal (a squatter add_crystal), tinted
    brighter toward the tip. Returns its faces for a per-piece normal recalc."""
    bx, by, bz = base_center
    sides = 5
    lean = math.radians(lean_deg)
    lean_axis = Vector((-math.sin(lean_az), math.cos(lean_az), 0.0))
    rot = Matrix.Rotation(lean, 4, lean_axis) if abs(lean_deg) > 1e-3 else Matrix.Identity(4)
    twist = hash01(seed, 7.0) * math.tau
    ring = []
    for i in range(sides):
        a = twist + i / sides * math.tau
        jr = base_r * (0.86 + 0.28 * hash01(seed + i, 3.0))
        local = rot @ Vector((math.cos(a) * jr, math.sin(a) * jr, 0.0))
        ring.append(bm.verts.new((bx + local.x, by + local.y, bz + local.z)))
    sh = []
    sh_z = height * 0.78
    for i in range(sides):
        a = twist + i / sides * math.tau
        jr = base_r * 0.34 * (0.8 + 0.4 * hash01(seed + i, 5.0))
        local = rot @ Vector((math.cos(a) * jr, math.sin(a) * jr, sh_z))
        sh.append(bm.verts.new((bx + local.x, by + local.y, bz + local.z)))
    apex_local = rot @ Vector(((hash01(seed, 1.0) - 0.5) * base_r * 0.3,
                               (hash01(seed, 2.0) - 0.5) * base_r * 0.3, height))
    apex = bm.verts.new((bx + apex_local.x, by + apex_local.y, bz + apex_local.z))
    # broken base cap so the shard has a solid mined bottom (not an open ring).
    base_local = rot @ Vector((0.0, 0.0, -base_r * 0.35))
    base_pt = bm.verts.new((bx + base_local.x, by + base_local.y, bz + base_local.z))
    bm.verts.ensure_lookup_table()
    faces = []
    for i in range(sides):
        j = (i + 1) % sides
        faces.append(bm.faces.new([ring[i], ring[j], sh[j], sh[i]]))  # body facet
        faces.append(bm.faces.new([sh[i], sh[j], apex]))              # tip facet
        faces.append(bm.faces.new([ring[j], ring[i], base_pt]))       # broken base
    bmesh.ops.recalc_face_normals(bm, faces=faces)
    tip_set = set(sh) | {apex}
    base_set = {base_pt}

    def color_of(v):
        if v in base_set:
            return C_DARK
        t = 0.75 if v in tip_set else 0.0
        return lerp3(C_MAIN, C_HI, t)
    for f in faces:
        f.smooth = False
        for lp in f.loops:
            c = color_of(lp.vert)
            lp[col] = (c[0], c[1], c[2], 1.0)
    return faces


def main():
    bpy.ops.wm.read_homefile(use_empty=True)
    bm = bmesh.new()
    col = bm.loops.layers.color.new("Color")

    # One hero shard, upright with a slight lean, plus two small satellite chips at
    # its base so the icon reads as a handful of the material (like the log stack /
    # coal cluster), not a lone spike. Squat proportions: a hand crystal.
    add_shard(bm, col, (0.0, 0.0, 0.0), height=0.92, base_r=0.30,
              lean_deg=6, lean_az=math.radians(20), seed=41.0)
    add_shard(bm, col, (-0.30, 0.10, -0.02), height=0.44, base_r=0.17,
              lean_deg=-24, lean_az=math.radians(200), seed=63.0)
    add_shard(bm, col, (0.28, -0.12, -0.02), height=0.34, base_r=0.15,
              lean_deg=22, lean_az=math.radians(60), seed=88.0)

    me = bpy.data.meshes.new("meteorite_shard")
    bm.to_mesh(me)
    bm.free()
    if me.color_attributes:
        idx = me.color_attributes.find("Color")
        if idx != -1:
            me.color_attributes.render_color_index = idx
            me.color_attributes.active_color_index = idx
    me.update()
    obj = bpy.data.objects.new("meteorite_shard", me)
    bpy.context.collection.objects.link(obj)
    mat = bpy.data.materials.new("meteorite_mat")
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = nt.nodes.get("Principled BSDF")
    vc = nt.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = "Color"
    if bsdf is not None:
        nt.links.new(vc.outputs["Color"], bsdf.inputs["Base Color"])
    obj.data.materials.append(mat)

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.export_scene.gltf(
        filepath=OUT, export_format="GLB", use_selection=True,
        export_yup=True, export_apply=True, export_normals=True,
        export_materials="EXPORT", export_texcoords=False,
    )
    print("wrote", OUT)


if __name__ == "__main__":
    main()
