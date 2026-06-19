#!/usr/bin/env python3
"""Turn a Draw-Things foliage patch (needles/leaves on a pale backdrop) into a
seamless, tileable RGBA texture for skinning tree canopy shells.

Steps (OpenCV + numpy only, no PIL):
  1. greenness key -> alpha (foreground = vegetation, background = pale grey)
  2. morphological clean (open then close) to kill specks + pinholes
  3. RGB color-bleed under transparent pixels (so mips/Mask show no dark halo)
  4. make seamless on BOTH axes (offset-and-blend, edge-weighted) incl. alpha
  5. optional saturation/brightness trim to seat in the muted palette
Output is mostly-opaque needles/leaves with a fine alpha fringe in the gaps;
the canopy bottom-rim feather is done in the MESH (vertex-colour alpha), not here.
"""
import argparse
import sys
from pathlib import Path

import cv2
import numpy as np


def log(m):
    print(m, file=sys.stderr, flush=True)


def greenness_alpha(bgr: np.ndarray, thr: float, lum_floor: int) -> np.ndarray:
    b, g, r = (bgr[:, :, i].astype(np.float32) for i in range(3))
    green = g - 0.5 * (r + b)               # vegetation is G-dominant
    lum = 0.114 * b + 0.587 * g + 0.299 * r
    # also catch dark needle shadows (low-lum, low-sat) that greenness misses
    a = np.zeros(bgr.shape[:2], np.float32)
    a[green > thr] = 1.0
    a[lum < lum_floor] = 1.0                 # dark interior needles
    # background is pale/bright + low greenness -> stays 0
    return a


def color_bleed(bgr: np.ndarray, alpha: np.ndarray, iters: int) -> np.ndarray:
    """Push opaque colour outward under transparent pixels so downsampling /
    Mask cutoff never reveals the (grey) background colour as a dark halo."""
    out = bgr.copy().astype(np.uint8)
    mask = (alpha > 0.5).astype(np.uint8)
    k = np.ones((3, 3), np.uint8)
    for _ in range(iters):
        dil = cv2.dilate(mask, k)
        edge = (dil > 0) & (mask == 0)
        if not edge.any():
            break
        blurred = cv2.blur(out, (3, 3))
        out[edge] = blurred[edge]
        mask = dil
    return out


def edge_weight(n, band):
    w = np.ones(n)
    ramp = np.linspace(0.0, 1.0, band)
    w[:band] = ramp
    w[-band:] = ramp[::-1]
    return w


def make_seamless(arr: np.ndarray) -> np.ndarray:
    h, w = arr.shape[:2]
    bw, bh = max(8, w // 4), max(8, h // 4)
    rolled = np.roll(arr, w // 2, axis=1)
    wx = edge_weight(w, bw)[None, :, None]
    horiz = arr * wx + rolled * (1.0 - wx)
    rolled2 = np.roll(horiz, h // 2, axis=0)
    wy = edge_weight(h, bh)[:, None, None]
    return horiz * wy + rolled2 * (1.0 - wy)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--src", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--thr", type=float, default=8.0, help="greenness threshold")
    p.add_argument("--lum-floor", type=int, default=70, help="dark needles below this lum -> opaque")
    p.add_argument("--feather", type=float, default=0.8, help="alpha edge feather px")
    p.add_argument("--sat", type=float, default=0.9, help="saturation scale")
    p.add_argument("--val", type=float, default=0.96, help="brightness scale")
    p.add_argument("--hue", type=float, default=0.0, help="additive hue shift, OpenCV 0-180 scale (+ = toward green)")
    p.add_argument("--degrey", type=float, default=0.0, help="saturation below this (0..1) -> blended to --degrey-color")
    p.add_argument("--degrey-color", type=int, nargs=3, default=[60, 95, 45], help="R G B target for de-grey (0-255)")
    p.add_argument("--size", type=int, default=512)
    p.add_argument("--no-seamless", action="store_true")
    args = p.parse_args()

    bgr = cv2.imread(args.src, cv2.IMREAD_COLOR)
    if bgr is None:
        log(f"cannot read {args.src}")
        sys.exit(1)
    a = greenness_alpha(bgr, args.thr, args.lum_floor)
    k = np.ones((3, 3), np.uint8)
    a = cv2.morphologyEx(a, cv2.MORPH_OPEN, k)
    a = cv2.morphologyEx(a, cv2.MORPH_CLOSE, k)
    bgr_bled = color_bleed(bgr, a, iters=12)
    if args.feather > 0:
        a = cv2.GaussianBlur(a, (0, 0), args.feather)
    a = np.clip(a, 0, 1)

    # De-grey: the generator sometimes leaves desaturated background bleed inside
    # the foliage mass; on an alpha-MASK material those opaque grey pixels read as
    # dirty grey needles. Blend every pixel toward a target foliage colour weighted
    # by how *grey* it is (low saturation), so grey patches become foliage and
    # already-coloured needles keep their hue/shading.
    if args.degrey:
        b, g, r = (bgr_bled[:, :, i].astype(np.float32) for i in range(3))
        mx = np.maximum(np.maximum(r, g), b)
        mn = np.minimum(np.minimum(r, g), b)
        sat = np.where(mx > 1, (mx - mn) / np.maximum(mx, 1), 0.0)  # 0..1
        w = np.clip((args.degrey - sat) / max(args.degrey, 1e-3), 0.0, 1.0)[..., None]
        tgt = np.array(args.degrey_color[::-1], np.float32)  # rgb -> bgr
        lum = (mx / 255.0)[..., None]                        # keep light/dark shading
        bgr_bled = (bgr_bled * (1 - w) + (tgt * (0.55 + 0.55 * lum)) * w).clip(0, 255).astype(np.uint8)

    # muted-palette trim in HSV (+ optional hue rotation toward green)
    hsv = cv2.cvtColor(bgr_bled, cv2.COLOR_BGR2HSV).astype(np.float32)
    if args.hue:
        hsv[:, :, 0] = (hsv[:, :, 0] + args.hue) % 180.0
    hsv[:, :, 1] *= args.sat
    hsv[:, :, 2] *= args.val
    hsv = np.clip(hsv, 0, 255).astype(np.uint8)
    rgb_trim = cv2.cvtColor(hsv, cv2.COLOR_HSV2BGR)

    bgra = np.dstack([rgb_trim, (a * 255).astype(np.uint8)]).astype(np.float64)
    if not args.no_seamless:
        bgra = make_seamless(bgra)
    bgra = np.clip(bgra, 0, 255).astype(np.uint8)
    if args.size:
        bgra = cv2.resize(bgra, (args.size, args.size), interpolation=cv2.INTER_AREA)

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    cv2.imwrite(str(out), bgra)
    cov = (bgra[:, :, 3] > 102).mean()
    log(f"  -> {out}  ({bgra.shape[1]}x{bgra.shape[0]}, {cov*100:.1f}% opaque @Mask0.4)")

    # checker preview + 2x2 tile preview to judge seam + density
    h, w = bgra.shape[:2]
    chk = np.indices((h, w)).sum(0) // 24 % 2
    chk = np.where(chk[..., None] == 0, 150, 90).astype(np.uint8).repeat(3, axis=2)
    al = bgra[:, :, 3:4].astype(np.float32) / 255
    comp = (bgra[:, :, :3] * al + chk * (1 - al)).astype(np.uint8)
    cv2.imwrite(str(out.with_name(out.stem + ".preview.png")), comp)
    tile = np.tile(comp, (2, 2, 1))
    cv2.imwrite(str(out.with_name(out.stem + ".tiled.png")), tile)


if __name__ == "__main__":
    main()
