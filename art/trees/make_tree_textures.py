#!/usr/bin/env python3
"""Generate the toony/cel tree textures with ComfyUI Flux and heal them into
seamless tiles with OpenCV. Mirrors the ore `rock_master.png` approach: a soft,
low-contrast hand-painted detail that rides on the mesh COLOR_0 (the cel shader
supplies the banding + ink edge, the texture only adds grain).

Four opaque 512px tiles (the new cel canopy is SOLID geometry, not alpha cards):
  bark_pine.png      reddish-brown furrowed cel bark
  bark_birch.png     white papery birch bark with black lenticel marks
  foliage_pine.png   dark-green needle grain (subtle)
  foliage_birch.png  light-green leaf grain (subtle)

Flux doesn't tile natively, so we generate at 1024, roll by half and feather the
centre cross (the only seam) toward the smooth interior, then knock the contrast
down so the texture never fights the cel bands. Writes straight into
assets/textures/trees/.

Usage:  python3 art/trees/make_tree_textures.py [name ...]   (default: all)
"""
import sys, os
import numpy as np
import cv2

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from comfy_gen import generate

OUT = "assets/textures/trees"
RAW = "/tmp/tree_tex_raw"
os.makedirs(OUT, exist_ok=True)
os.makedirs(RAW, exist_ok=True)

STYLE = ("seamless tileable texture, top-down orthographic flat surface, "
         "hand-painted cel-shaded anime game texture, flat even ambient light, "
         "no hotspots, no directional shadow, uniform, low contrast")

# name -> (prompt, seed, contrast 0..1 toward mid-grey, target tint or None)
TEX = {
    "bark_pine": (
        "pine conifer tree bark, vertical furrowed reddish brown plates and ridges, "
        "warm sienna and umber, stylized chunky grooves", 11, 0.45, None),
    "bark_birch": (
        "birch tree bark, smooth white papery surface with short horizontal black "
        "lenticel dashes and faint grey shading, clean stylized", 23, 0.40, None),
    "foliage_pine": (
        "dense pine needle sprigs and conifer fronds, deep forest green, soft "
        "clustered needles, gentle painted shading", 31, 0.62, None),
    "foliage_birch": (
        "small rounded birch leaves clustered together, fresh light green, soft "
        "leafy clumps, gentle painted shading", 37, 0.60, None),
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
    """Pull contrast toward the image's own mean so the tile reads as flat
    detail under the cel bands (amount 0 = unchanged, 1 = flat)."""
    f = img.astype(np.float32)
    mean = f.reshape(-1, 3).mean(0)[None, None, :]
    return np.clip(f * (1 - amount) + mean * amount, 0, 255).astype(np.uint8)


def build(name):
    prompt, seed, contrast, _ = TEX[name]
    raw = f"{RAW}/{name}.png"
    generate(f"{prompt}, {STYLE}", raw, 1024, 1024, 4, seed)
    img = cv2.imread(raw)
    img = make_seamless(img)
    img = soften(img, contrast)
    img = cv2.resize(img, (512, 512), interpolation=cv2.INTER_AREA)
    dst = f"{OUT}/{name}.png"
    cv2.imwrite(dst, img)
    print(f"{name}: {dst}")


if __name__ == "__main__":
    names = sys.argv[1:] or list(TEX)
    for n in names:
        build(n)
