#!/usr/bin/env python3
"""Picked unified candidates -> art/items/<id>/icon_master_512.png + game icons.

Round-2 flow: ONE picked image per item serves as both the mesh reference and
the icon. This script derives the icon side: read selection.json, take each
picked candidate, auto-correct mirroring (the inventory convention is head at
the UPPER LEFT; generations sometimes arrive flipped, and the head is detected
as the heavier-alpha side of the top band), keep only the largest connected
alpha component (drops any stray painted shadow blob), recenter with the icon
margin, write the 512 master, then run scripts/icon_finalize.py for the
in-game 160 px icon.

Run:  python3 make_icon_masters.py [--only <item_id>] [--no-finalize]
"""
import argparse
import json
import subprocess
import sys
from pathlib import Path

import cv2
import numpy as np

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
SELECTION = HERE / "selection.json"
PROMPTS = HERE / "prompts.json"
FINALIZE = REPO / "scripts" / "icon_finalize.py"
MARGIN = 0.10


def band_width(img: np.ndarray, top: bool) -> float:
    """Horizontal alpha spread of the top (or bottom) 35% of the content."""
    alpha = img[:, :, 3]
    ys, xs = np.nonzero(alpha > 32)
    span = ys.max() - ys.min()
    if top:
        sel = ys <= ys.min() + int(0.35 * span)
    else:
        sel = ys >= ys.max() - int(0.35 * span)
    band = xs[sel]
    return float(band.max() - band.min()) if band.size else 0.0


def head_is_bottom(img: np.ndarray) -> bool:
    """The head end is the WIDER end; if the bottom band CLEARLY out-spans
    the top, the composition arrived upside down. The 1.3x margin keeps
    near-symmetric silhouettes (a wide flared pommel under a compact head)
    from false-triggering; a wrong pass-through is a visible fix, a wrong
    rotation is an upside-down icon."""
    return band_width(img, top=False) > 1.3 * band_width(img, top=True)


def head_is_right(img: np.ndarray) -> bool:
    """The head is the mass concentration in the top band of the diagonal
    composition; if it sits right of centre the image is mirrored."""
    alpha = img[:, :, 3]
    ys, xs = np.nonzero(alpha > 32)
    top_cut = ys.min() + int(0.40 * (ys.max() - ys.min()))
    top_xs = xs[ys <= top_cut]
    mid = (xs.min() + xs.max()) / 2
    return float(np.mean(top_xs)) > mid


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--only")
    p.add_argument("--no-finalize", action="store_true")
    args = p.parse_args()

    selection = json.loads(SELECTION.read_text())
    spec = json.loads(PROMPTS.read_text())
    meta = {i["key"]: i for i in spec["items"]}
    for item_id, pick in selection.items():
        if args.only and item_id != args.only:
            continue
        item = meta[item_id]
        # The generic bundle is a viewmodel-only pick; no item, no icon.
        if item["group"] == "generic":
            continue
        src = HERE / "candidates" / f"item_{item_id}_v{pick}.png"
        img = cv2.imread(str(src), cv2.IMREAD_UNCHANGED)
        assert img is not None and img.shape[2] == 4, f"bad {src}"

        # The head-upper-left convention (and its rotate/mirror repair) only
        # applies to the DIAGONAL haft compositions; a front-on door or a
        # three-quarter chest must never be flipped by a band heuristic.
        if "diagonally" in item["orient"]:
            if head_is_bottom(img):
                img = img[::-1, ::-1].copy()
                print(f"{item_id}: rotated 180 (head was at the bottom)")
            if head_is_right(img):
                img = img[:, ::-1].copy()
                print(f"{item_id}: flipped (head was upper-right)")

        # Drop stray blobs (painted shadow fragments, floating specks), but
        # keep every component of comparable size to the largest: armor pairs
        # (two boots, two greaves) and material clusters are legitimately
        # disconnected silhouettes.
        alpha = img[:, :, 3]
        mask = (alpha > 32).astype(np.uint8)
        n, labels = cv2.connectedComponents(mask, connectivity=8)
        if n > 2:
            sizes = np.array([(labels == i).sum() for i in range(1, n)])
            drop = [i + 1 for i, s in enumerate(sizes)
                    if s < 0.15 * sizes.max()]
            if drop:
                img[np.isin(labels, drop) & (mask > 0), 3] = 0
                print(f"{item_id}: dropped {len(drop)} stray blob(s)")

        ys, xs = np.nonzero(img[:, :, 3] > 8)
        crop = img[ys.min():ys.max() + 1, xs.min():xs.max() + 1]
        side = int(max(crop.shape[:2]) / (1.0 - 2 * MARGIN))
        canvas = np.zeros((side, side, 4), dtype=np.uint8)
        oy = (side - crop.shape[0]) // 2
        ox = (side - crop.shape[1]) // 2
        canvas[oy:oy + crop.shape[0], ox:ox + crop.shape[1]] = crop
        out512 = cv2.resize(canvas, (512, 512), interpolation=cv2.INTER_AREA)

        master = REPO / "art" / "items" / item_id / "icon_master_512.png"
        master.parent.mkdir(parents=True, exist_ok=True)
        cv2.imwrite(str(master), out512)
        print(f"{item_id}: master written from {src.name}")

        if not args.no_finalize:
            icon = REPO / "assets" / "items" / item_id / "icon.png"
            r = subprocess.run([sys.executable, str(FINALIZE),
                                "--master", str(master), "--out", str(icon)])
            if r.returncode != 0:
                print(f"{item_id}: FINALIZE FAILED", file=sys.stderr)


if __name__ == "__main__":
    main()
