#!/usr/bin/env python3
"""Generate the toony/cel HAND-TOOL detail textures with ComfyUI Flux and heal
them into seamless tiles with OpenCV. Mirrors the tree/ore approach: a soft,
low-contrast hand-painted grain that rides on the mesh COLOR_0 (the cel shader
supplies the banding + ink edge; the texture only adds material detail).

Four opaque 512px tiles feed the four tools (body prim + head prim):
  wood.png    warm turned-wood haft grain (both pickaxe + hatchet hafts)
  stone.png   knapped grey stone head (stone pickaxe + stone hatchet)
  iron.png    forged blue-grey steel head (iron pickaxe + iron hatchet)
  twine.png   tan fibre lashing (haft binding, rides COLOR_0 on body prim)

Flux doesn't tile natively, so we generate at 1024, roll by half and feather the
centre cross (the only seam) toward the smooth interior, then knock the contrast
down so the texture never fights the cel bands. Writes into assets/textures/tools/.

Workflow:
  python3 art/tools/make_tool_textures.py raw            # all variants -> /tmp
  python3 art/tools/make_tool_textures.py raw wood        # one material's variants
  python3 art/tools/make_tool_textures.py final wood wood_b 0.50   # pick + heal
"""
import sys, os
import numpy as np
import cv2

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from comfy_gen import generate

OUT = "assets/textures/tools"
RAW = "/tmp/tool_tex_raw"
os.makedirs(OUT, exist_ok=True)
os.makedirs(RAW, exist_ok=True)

STYLE = ("seamless tileable texture, top-down orthographic flat surface, "
         "hand-painted cel-shaded anime game texture, flat even ambient light, "
         "no hotspots, no directional shadow, uniform, low contrast")

# material -> list of (variant_suffix, prompt, seed)
VARIANTS = {
    "wood": [
        ("a", "smooth turned wooden tool handle shaft, straight vertical wood "
              "grain, warm honey oak, faint growth lines", 41),
        ("b", "polished ash wood tool handle, fine straight grain, light golden "
              "brown, subtle figure and streaks", 42),
        ("c", "worn hardwood tool haft, warm reddish-brown walnut grain, gentle "
              "lengthwise streaks", 43),
        ("d", "smooth carved wood handle, pale warm tan grain, clean even surface, "
              "soft growth rings", 44),
    ],
    "stone": [
        ("a", "knapped grey flint stone surface, chipped conchoidal facets, matte "
              "light stone grey, subtle chisel marks", 51),
        ("b", "rough granite tool head surface, speckled mid grey rock, subtle "
              "pitting and grain", 52),
        ("c", "chiselled basalt stone, cool neutral grey, sharp faceted chips", 53),
        ("d", "smooth grey river stone rock surface, soft matte, gentle mottling", 54),
    ],
    "iron": [
        ("a", "forged iron tool head surface, hammered dark steel, subtle hammer "
              "dents, cool blue-grey metal", 61),
        ("b", "polished wrought iron, smooth grey steel with faint scratches, "
              "slight cool sheen", 62),
        ("c", "rough cast iron surface, dark charcoal grey metal, faint mottling", 63),
        ("d", "weathered steel axe head, brushed metal grain, cool silver grey, "
              "light patina", 64),
    ],
    "twine": [
        ("a", "natural jute twine cord wrapped in tight parallel bands, tan "
              "fibrous rope strands, stylized", 71),
        ("b", "woven hemp cord lashing, warm beige fiber strands, neat tight wrap", 72),
        ("c", "leather strap wrap, warm mid-brown hide binding, stylized", 73),
    ],
    # Smooth/toony retake of the heads: big broad rounded forms + soft wide seams
    # (like the ore node `rock.png`), NOT lots of small cobbles. Matches the
    # smooth ground-stone look the art direction wants.
    "stone2": [
        ("a", "large smooth grey stone boulder surface, a few broad rounded "
              "plates, soft wide gentle cracks, very smooth clean interior, "
              "hand-painted toony anime, soft even shading", 81),
        ("b", "smooth polished grey rock surface, gentle large forms, minimal "
              "soft seams, clean stylized anime stone, smooth matte", 82),
        ("c", "smooth carved grey stone, soft subtle mottling, faint broad "
              "seams, clean toony game texture, even light", 83),
        ("d", "smooth grey granite slab, very soft large patches, barely visible "
              "cracks, clean anime stone, flat soft light", 84),
    ],
    "iron2": [
        ("a", "smooth forged steel surface, soft gentle value gradient, very "
              "faint long scratches, clean toony anime metal, smooth matte grey", 91),
        ("b", "smooth polished iron plate, subtle soft sheen variation, minimal "
              "marks, clean stylized cool grey metal", 92),
        ("c", "smooth blued steel surface, soft broad highlights, faint planishing, "
              "clean toony anime, cool grey", 93),
    ],
    # Parchment / paper for the rolled building-plan scroll. Soft fiber grain,
    # cream-tan; COLOR_0 supplies the cream paper vs the brown twine ties.
    "parchment": [
        ("a", "old parchment paper surface, subtle horizontal fiber grain, soft "
              "mottled cream and tan, faint age stains, hand-painted toony anime, "
              "flat even light", 101),
        ("b", "aged vellum scroll paper, gentle fibrous texture, warm cream beige, "
              "soft subtle blotches, clean stylized, flat light", 102),
        ("c", "rough handmade paper sheet, soft speckled grain, light tan, faint "
              "soft creases, clean anime game texture", 103),
    ],
}


def make_seamless(img, feather=0.34):
    """Roll by half so the tile edges become smooth interior, then feather the
    resulting centre cross (the former edges) toward the interior."""
    img = img.astype(np.float32)
    h, w = img.shape[:2]
    rolled = np.roll(img, (h // 2, w // 2), axis=(0, 1))
    yy = np.abs(np.arange(h) - h / 2) / (h / 2)
    xx = np.abs(np.arange(w) - w / 2) / (w / 2)
    wy = np.clip(yy / feather, 0, 1)
    wx = np.clip(xx / feather, 0, 1)
    weight = np.minimum(wy[:, None], wx[None, :])[..., None]  # 0 at cross, 1 at edge
    out = rolled * weight + img * (1.0 - weight)
    return np.clip(out, 0, 255).astype(np.uint8)


def soften(img, amount):
    """Pull contrast toward the image's own mean so the tile reads as flat detail
    under the cel bands (amount 0 = unchanged, 1 = flat)."""
    f = img.astype(np.float32)
    mean = f.reshape(-1, 3).mean(0)[None, None, :]
    return np.clip(f * (1 - amount) + mean * amount, 0, 255).astype(np.uint8)


def to_detail(img, desat=0.85, target_mean=210.0):
    """Turn a coloured Flux tile into a near-neutral DETAIL grain map: the tool
    cel-shading multiplies `texture * COLOR_0`, so the mesh COLOR_0 owns the
    colour (wood brown / stone grey / iron steel) and the texture must only carry
    light value grain. Pull most of the saturation out (toward luma) and rescale
    the mean luminance to a light neutral so it never darkens or hue-shifts the
    COLOR_0. Mirrors the ore `rock.png` (neutral grey, mean ~0.8) approach."""
    f = img.astype(np.float32)
    luma = (f * np.array([0.114, 0.587, 0.299])).sum(2, keepdims=True)  # BGR weights
    f = f * (1.0 - desat) + luma * desat
    m = f.mean()
    if m > 1e-3:
        f *= target_mean / m
    return np.clip(f, 0, 255).astype(np.uint8)


def gen_raw(materials):
    for m in materials:
        for suffix, prompt, seed in VARIANTS[m]:
            raw = f"{RAW}/{m}_{suffix}.png"
            generate(f"{prompt}, {STYLE}", raw, 1024, 1024, 4, seed)
            print(f"raw {m}_{suffix}: {raw}", flush=True)


def finalize(material, variant, contrast, desat=0.85, target_mean=210.0):
    raw = f"{RAW}/{variant}.png" if not variant.endswith(".png") else variant
    img = cv2.imread(raw)
    img = make_seamless(img)
    img = to_detail(img, desat=desat, target_mean=target_mean)
    img = soften(img, contrast)
    img = cv2.resize(img, (512, 512), interpolation=cv2.INTER_AREA)
    dst = f"{OUT}/{material}.png"
    cv2.imwrite(dst, img)
    print(f"final {material}: {dst}  (from {raw}, contrast {contrast}, "
          f"desat {desat}, mean {target_mean})")


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "raw"
    if mode == "raw":
        mats = sys.argv[2:] or list(VARIANTS)
        gen_raw(mats)
    elif mode == "final":
        material = sys.argv[2]
        variant = sys.argv[3]
        contrast = float(sys.argv[4]) if len(sys.argv) > 4 else 0.50
        finalize(material, variant, contrast)
    else:
        print(f"unknown mode {mode!r}; use 'raw' or 'final'")
