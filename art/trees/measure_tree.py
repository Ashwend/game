#!/usr/bin/env python3
"""Measure a tree reference silhouette (OpenCV) into the proportions the
parametric builder needs, so the toony glb matches the concept instead of being
eyeballed. Mirrors the ore pipeline's /tmp/measure_ore.py step.

Input: a single-tree reference on a flat white background (ComfyUI Flux, see
art/comfy_gen.py). We threshold to a foreground mask (coloured tree body + dark
ink outline, dropping the light-grey ambient shadow by saturation), then walk the
mask row by row to get a width-vs-height profile. From that we extract the
landmarks the builder keys off:

  total_h        full silhouette height (px)
  trunk_w_frac   trunk width / total_h   (drives base trunk radius)
  canopy_bot     height fraction where the canopy starts (width jumps up)
  canopy_peak    height fraction of the widest canopy row
  canopy_w_frac  max canopy half-width / total_h  (drives canopy max radius)
  canopy_top     height fraction where foliage ends (~1.0)
  apex_taper     how pointed the top is (conifer ~ sharp, birch ~ round)

All as fractions of total height, so the same numbers scale to any in-game tree
height (pine 4.5/6.6/9.1 m, birch 3.6/5.3/7.15 m; see components.rs).

Usage:
  python3 art/trees/measure_tree.py art/trees/refs/pine_c1.png [debug_out.png]
"""
import sys
import cv2
import numpy as np


def foreground_mask(bgr):
    """Tree body (coloured) + dark ink outline; excludes light-grey shadow."""
    b, g, r = bgr[..., 0].astype(int), bgr[..., 1].astype(int), bgr[..., 2].astype(int)
    mx = np.maximum(np.maximum(b, g), r)
    mn = np.minimum(np.minimum(b, g), r)
    sat = mx - mn                     # chroma; tree is coloured, shadow is grey
    val = mx                          # brightness; ink outline is dark
    fg = ((sat > 16) | (val < 205)) & (val < 250)
    m = (fg.astype(np.uint8)) * 255
    # keep the largest connected blob (drops detached shadow / stray marks)
    n, lbl, stats, _ = cv2.connectedComponentsWithStats(m, 8)
    if n > 1:
        big = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
        m = np.where(lbl == big, 255, 0).astype(np.uint8)
    # close small holes so interior gaps don't break the width profile
    m = cv2.morphologyEx(m, cv2.MORPH_CLOSE, np.ones((5, 5), np.uint8))
    return m


def profile(mask):
    """Per-row (top->bottom) half-width and centre x, in px."""
    ys = np.where(mask.any(axis=1))[0]
    y0, y1 = ys.min(), ys.max()
    rows = []
    for y in range(y0, y1 + 1):
        xs = np.where(mask[y] > 0)[0]
        if len(xs) == 0:
            rows.append((0.0, None))
        else:
            rows.append(((xs.max() - xs.min()) / 2.0, (xs.max() + xs.min()) / 2.0))
    return y0, y1, rows


def measure(path, debug=None):
    bgr = cv2.imread(path)
    if bgr is None:
        raise SystemExit(f"cannot read {path}")
    H, W = bgr.shape[:2]
    mask = foreground_mask(bgr)
    y0, y1, rows = profile(mask)
    total_h = y1 - y0
    hw = np.array([hw for hw, _ in rows])          # half-width per row, top->bottom
    if total_h < 10 or hw.max() <= 0:
        raise SystemExit("degenerate silhouette")

    peak_i = int(np.argmax(hw))
    peak_hw = hw[peak_i]
    # height fraction measured from the BASE (bottom = 0, top = 1)
    def frac_from_base(i):  # i is index from top
        return 1.0 - (i / total_h)

    canopy_peak = frac_from_base(peak_i)
    canopy_w_frac = peak_hw / total_h

    # trunk width: median half-width of the bottom 12% of rows that are clearly
    # narrower than the canopy (the bare stem). Fall back to bottom rows.
    bottom = hw[int(total_h * 0.88):]
    stem = bottom[bottom < peak_hw * 0.5]
    trunk_hw = float(np.median(stem)) if len(stem) >= 3 else float(np.median(bottom))
    trunk_w_frac = trunk_hw / total_h

    # canopy bottom: walking up from the base, first row whose width exceeds
    # 1.8x the trunk half-width (foliage flares out)
    thresh = max(trunk_hw * 1.8, peak_hw * 0.18)
    canopy_bot = 0.0
    for i in range(len(hw) - 1, -1, -1):           # base -> top
        if hw[i] > thresh:
            canopy_bot = frac_from_base(i)
            break

    # apex taper: half-width at 92% height / peak half-width (small => pointed)
    i92 = int((1.0 - 0.92) * total_h)
    apex_taper = float(hw[max(0, min(i92, len(hw) - 1))] / peak_hw)

    canopy_top = frac_from_base(int(np.argmax(hw > 0)))  # topmost non-empty row

    res = dict(
        src=path, px=f"{W}x{H}", total_h=int(total_h),
        trunk_w_frac=round(trunk_w_frac, 4),
        canopy_bot=round(canopy_bot, 3),
        canopy_peak=round(canopy_peak, 3),
        canopy_w_frac=round(canopy_w_frac, 4),
        canopy_top=round(canopy_top, 3),
        apex_taper=round(apex_taper, 3),
        aspect_canopy=round(peak_hw * 2 / total_h, 3),
    )

    if debug:
        vis = bgr.copy()
        vis[mask > 0] = (vis[mask > 0] * 0.5 + np.array([0, 0, 255]) * 0.5).astype(np.uint8)
        for i, (h_w, cx) in enumerate(rows):
            if cx is not None and i % 6 == 0:
                y = y0 + i
                cv2.line(vis, (int(cx - h_w), y), (int(cx + h_w), y), (0, 255, 0), 1)
        cv2.imwrite(debug, vis)
    return res


if __name__ == "__main__":
    p = sys.argv[1]
    dbg = sys.argv[2] if len(sys.argv) > 2 else None
    r = measure(p, dbg)
    w = max(len(k) for k in r)
    for k, v in r.items():
        print(f"  {k:<{w}} : {v}")
