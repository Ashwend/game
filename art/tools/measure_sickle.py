#!/usr/bin/env python3
"""Measure the chosen sickle icon's silhouette for the model build.

Per the art pipeline (docs/playbooks/art-pipeline.md): the icon comes first
(ComfyUI), the model is built to MATCH it, and the match is MEASURED with
OpenCV, never eyeballed. This script splits the RGBA cutout into handle and
blade, fits a circle to the blade crescent, then sweeps the blade's angular
extent to emit the centreline path + ribbon half-width per station in
HANDLE-RELATIVE units, ready to paste into build_sickle.py (which scales
them by its authored haft length).

Segmentation: the tool is split SPATIALLY at the collar, not by hue. (The
first version segmented blade-vs-handle by saturation; the icon3_v6 blade's
rust patches are warm and its shadowed wood is dark, so hue rules misfile
both.) The collar-top and butt-tip anchors are passed on the command line,
read off the image by eye once (they are unambiguous landmarks); everything
downstream is measured. The handle frame is: origin at the collar anchor,
+z toward the collar from the butt, +x the perpendicular toward the blade's
bulk (the build script's forward axis).

Usage:
  python3 art/tools/measure_sickle.py <cutout.png> --collar 707,407 --butt 432,941
"""

import argparse

import cv2
import numpy as np


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("image", help="RGBA cutout of the icon")
    ap.add_argument("--collar", required=True, help="collar-top pixel, x,y")
    ap.add_argument("--butt", required=True, help="butt-tip pixel, x,y")
    ap.add_argument("--stations", type=int, default=12)
    args = ap.parse_args()

    img = cv2.imread(args.image, cv2.IMREAD_UNCHANGED)
    assert img is not None and img.shape[2] == 4, "need an RGBA cutout"
    alpha = img[:, :, 3] > 128

    collar = np.array([float(v) for v in args.collar.split(",")], dtype=np.float32)
    butt = np.array([float(v) for v in args.butt.split(",")], dtype=np.float32)
    up = collar - butt
    handle_len = float(np.linalg.norm(up))
    up /= handle_len
    minor = np.array([-up[1], up[0]], dtype=np.float32)
    print(f"handle: len {handle_len:.1f} px (collar anchor to butt anchor)")

    py, px = np.nonzero(alpha)
    pts = np.stack([px, py], axis=1).astype(np.float32)
    side = (pts - collar) @ up
    bpts = pts[side > 15.0]     # past the collar = blade
    hpts = pts[side < -15.0]    # before it = handle
    hw = np.abs((hpts - hpts.mean(axis=0)) @ minor)
    print(f"handle width {np.percentile(hw, 90) * 2.0:.1f} px "
          f"(J-curve deviation included; sanity only)")

    # Kasa algebraic circle fit on the blade pixels.
    a_mat = np.column_stack([bpts[:, 0], bpts[:, 1], np.ones(len(bpts))])
    b_vec = (bpts**2).sum(axis=1)
    sol, *_ = np.linalg.lstsq(a_mat, b_vec, rcond=None)
    cx, cy = sol[0] / 2.0, sol[1] / 2.0
    radius = float(np.sqrt(sol[2] + cx * cx + cy * cy))
    centre = np.array([cx, cy], dtype=np.float32)
    print(f"crescent fit: radius/handle {radius / handle_len:.3f}")

    rel = bpts - centre
    r = np.linalg.norm(rel, axis=1)
    theta = np.arctan2(rel[:, 1], rel[:, 0])
    # Unwrap about the circular mean so a span crossing -pi/pi stays whole.
    mean_angle = np.arctan2(np.sin(theta).mean(), np.cos(theta).mean())
    theta = (theta - mean_angle + np.pi) % (2.0 * np.pi) - np.pi
    t_lo, t_hi = np.percentile(theta, 1.0), np.percentile(theta, 99.0)
    print(f"arc span: {np.degrees(t_hi - t_lo):.1f} deg")

    # +x toward the blade tip's side of the handle axis.
    tip_side = np.sign(float(((bpts - collar) @ minor).mean()))
    fwd = minor * tip_side

    stations = []
    n = args.stations
    for k in range(n):
        a = t_lo + (t_hi - t_lo) * k / (n - 1)
        band = np.abs(theta - a) < max(np.radians(3.0), (t_hi - t_lo) / (2 * n))
        if band.sum() < 12:
            continue
        rb = r[band]
        r_in, r_out = np.percentile(rb, 5.0), np.percentile(rb, 95.0)
        mid = centre + np.array(
            [np.cos(a + mean_angle), np.sin(a + mean_angle)], dtype=np.float32
        ) * ((r_in + r_out) / 2.0)
        fx = float((mid - collar) @ fwd) / handle_len
        fz = float((mid - collar) @ up) / handle_len
        stations.append((fx, fz, (r_out - r_in) / 2.0 / handle_len))

    # Order root -> tip (root = the station nearest the collar anchor).
    if np.hypot(*stations[0][:2]) > np.hypot(*stations[-1][:2]):
        stations.reverse()
    print("stations root->tip (x_forward, z_up, half_width), / handle length:")
    for fx, fz, hw_ in stations:
        print(f"  ({fx: .4f}, {fz: .4f}, {hw_:.4f}),")
    ccx = float((centre - collar) @ fwd) / handle_len
    ccz = float((centre - collar) @ up) / handle_len
    print(f"arc centre (handle frame): ({ccx:.4f}, {ccz:.4f})")


if __name__ == "__main__":
    main()
