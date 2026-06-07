#!/usr/bin/env python3
"""Convert an item-icon master (e.g. 512px) into the game icon (default 160px).

Companion to the lowpoly-game-assets skill. The skill's generate.py produces the
master under art/items/<id>/icon_master_512.png; this script does the master ->
assets/items/<id>/icon.png step with the downscale done safely so icons do not
alias in-game.

Why this exists: egui user textures have NO mipmaps, so the inventory/actionbar
minifies each 160px icon ~3.3x into its slot with plain bilinear. Two things make
that sparkle:
  1) Ringing filters. ImageMagick's default downscale filter (and Lanczos) have
     negative lobes that overshoot on soft or high-contrast sources, spiking the
     per-pixel gradient and adding bright edge speckles. We use Triangle (linear),
     which has no negative lobes.
  2) Garbage RGB in transparent/fringe pixels. Straight-alpha bilinear interpolates
     RGB and alpha separately, so undefined RGB under the silhouette bleeds in. We
     edge-bleed (dilate) the opaque RGB outward first, so every fringe texel carries
     the silhouette color.

Those two are SAFE for every icon (invisible on already-clean ones) and are always
applied. The pickaxe-specific knobs are opt-in and must NOT be used blanket:
  --desaturate CAP : clamp opaque saturation (kills stray saturated artifact pixels).
                     NEVER use on intentionally colorful icons (ores, furnace ember).
  --smooth SIGMA   : gaussian pre-blur to tame an over-detailed source. Prefer fixing
                     detail at the source (img2img via the lowpoly-game-assets skill's
                     `gen_icon_ref.py --steps 1`).
  --despeckle DEV  : median-replace isolated outlier pixels deviating > DEV from their
                     3x3 median (removes lone hot pixels).

QA: prints the finished icon's mean RGB gradient and opaque saturation as context.
Grad is a rough detail gauge (clean set ~1.8-2.7), but it does NOT by itself predict
in-game sparkle: organic icons (fiber 4.3, plant_twine 6.6) carry lots of detail yet
look fine, because their aliasing reads as plausible texture. The iron pickaxe (5.36)
looked broken because its aliasing put bright-white specks on a DARK matte head, which
pops perceptually. So a high grad is only a concern when the icon is ALSO high-contrast
(near-black next to near-white, e.g. bare metal). The reliable check is still visual:
view the slot in-game at the real Retina scale next to a known-good neighbor.

Usage:
  python3 scripts/icon_finalize.py --master art/items/iron_pickaxe/icon_master_512.png \
    --out assets/items/iron_pickaxe/icon.png
  # colorful icon, just the safe pipeline (default):
  python3 scripts/icon_finalize.py --master art/items/sulfur_ore/icon_master_512.png \
    --out assets/items/sulfur_ore/icon.png
  # problem icon with a stray saturated pixel:
  python3 scripts/icon_finalize.py --master M.png --out O.png --desaturate 0.55 --despeckle 45
"""

import argparse
import os
import subprocess
import sys
import tempfile

import numpy as np


def magick(*args):
    subprocess.run(["magick", *args], check=True)


def magick_out(*args):
    return subprocess.run(["magick", *args], check=True, capture_output=True, text=True).stdout


def load_rgba(png):
    w, h = (int(v) for v in magick_out(png, "-format", "%w %h", "info:").split())
    raw = tempfile.mktemp(suffix=".rgba")
    try:
        magick(png, "-depth", "8", f"rgba:{raw}")
        return np.fromfile(raw, dtype=np.uint8).reshape(h, w, 4).astype(np.float64)
    finally:
        if os.path.exists(raw):
            os.unlink(raw)


def save_rgba(arr, out):
    h, w = arr.shape[:2]
    raw = tempfile.mktemp(suffix=".rgba")
    try:
        np.clip(arr, 0, 255).astype(np.uint8).tofile(raw)
        magick("-depth", "8", "-size", f"{w}x{h}", f"rgba:{raw}", out)
    finally:
        if os.path.exists(raw):
            os.unlink(raw)


def clamp_saturation(rgb, cap):
    """Pull chroma toward grey wherever HSV saturation exceeds cap (V and H kept)."""
    mx = rgb.max(2, keepdims=True)
    mn = rgb.min(2, keepdims=True)
    sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1e-6), 0.0)
    scale = np.where(sat > cap, cap / np.maximum(sat, 1e-6), 1.0)
    return mx - (mx - rgb) * scale


def despeckle(rgb, dev_thresh):
    """Replace pixels deviating > dev_thresh from their 3x3 median (lone hot pixels)."""
    shifts = [np.roll(np.roll(rgb, dy, 0), dx, 1) for dy in (-1, 0, 1) for dx in (-1, 0, 1)]
    med = np.median(np.stack(shifts, 0), axis=0)
    dev = np.abs(rgb - med).max(2, keepdims=True)
    return np.where(dev > dev_thresh, med, rgb)


def edge_bleed(rgb, alpha, iters=48, known_at=16):
    """Dilate opaque RGB outward into transparent pixels so straight-alpha bilinear
    never interpolates undefined color across the silhouette."""
    filled = rgb.copy()
    known = alpha > known_at
    for _ in range(iters):
        if known.all():
            break
        s = np.zeros_like(filled)
        c = np.zeros(filled.shape[:2])
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                if dy == 0 and dx == 0:
                    continue
                s += np.roll(np.roll(filled * known[..., None], dy, 0), dx, 1)
                c += np.roll(np.roll(known.astype(float), dy, 0), dx, 1)
        new = (~known) & (c > 0)
        filled[new] = s[new] / c[new, None]
        known = known | new
    return filled


def grad_and_sat(arr):
    rgb = arr[:, :, :3]
    a = arr[:, :, 3]
    gx = np.abs(np.diff(rgb, axis=1)).mean()
    gy = np.abs(np.diff(rgb, axis=0)).mean()
    op = a > 200
    if op.any():
        r, g, b = rgb[:, :, 0][op], rgb[:, :, 1][op], rgb[:, :, 2][op]
        mx = np.maximum(np.maximum(r, g), b)
        mn = np.minimum(np.minimum(r, g), b)
        sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1), 0.0)
        smax = float(sat.max())
    else:
        smax = 0.0
    return (gx + gy) / 2, smax


def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--master", required=True, help="Source master PNG (any size)")
    p.add_argument("--out", required=True, help="Output game icon PNG")
    p.add_argument("--size", type=int, default=160, help="Output square size (default 160)")
    p.add_argument("--desaturate", type=float, default=None,
                   help="Opt-in: clamp opaque saturation to this cap (e.g. 0.55). "
                        "NEVER use on colorful icons (ores, ember).")
    p.add_argument("--smooth", type=float, default=None,
                   help="Opt-in: gaussian pre-blur sigma (master res) for over-detailed sources")
    p.add_argument("--despeckle", type=float, default=None,
                   help="Opt-in: median-replace pixels deviating > this from their 3x3 median")
    p.add_argument("--warn-grad", type=float, default=4.0,
                   help="Advisory note if finished grad exceeds this (default 4.0). "
                        "Grad is not a precise sparkle predictor; see module docstring.")
    args = p.parse_args()

    m = load_rgba(args.master)
    rgb = m[:, :, :3].copy()
    alpha = m[:, :, 3].copy()

    # --- opt-in cleanups (pickaxe-specific; off by default) ---
    if args.despeckle is not None:
        rgb = despeckle(rgb, args.despeckle)
    if args.desaturate is not None:
        rgb = clamp_saturation(rgb, args.desaturate)

    # --- always-on safe transforms ---
    rgb = edge_bleed(rgb, alpha)
    bled = m.copy()
    bled[:, :, :3] = rgb

    tmp_in = tempfile.mktemp(suffix=".png")
    tmp_out = tempfile.mktemp(suffix=".png")
    try:
        save_rgba(bled, tmp_in)
        resize_args = ["magick", tmp_in, "-filter", "Triangle"]
        if args.smooth is not None:
            resize_args += ["-blur", f"0x{args.smooth}"]
        resize_args += ["-resize", f"{args.size}x{args.size}", tmp_out]
        subprocess.run(resize_args, check=True)
        out = load_rgba(tmp_out)
    finally:
        for f in (tmp_in, tmp_out):
            if os.path.exists(f):
                os.unlink(f)

    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    save_rgba(out, args.out)

    grad, smax = grad_and_sat(out)
    note = "  (high detail; only a sparkle risk if also high-contrast, verify in-game)" \
        if grad > args.warn_grad else ""
    print(f"{args.out}: {out.shape[1]}x{out.shape[0]}  grad~{grad:.2f}  opaque maxSat={smax:.2f}{note}",
          file=sys.stderr)


if __name__ == "__main__":
    main()
