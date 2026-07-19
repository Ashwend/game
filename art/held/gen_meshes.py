#!/usr/bin/env python3
"""Stage 2: take the picker's selection, build meshes, write the review page.

Reads selection.json (the blob copied out of pick.html; round-2 shape is
`{"<item_id>": <variant>}` because ONE picked image serves as both the icon
and the mesh reference), and for each item:
  1. submits the picked image to the RunPod TRELLIS.2 worker,
  2. renders a 4-angle turntable of the resulting .glb in Blender,
  3. collects triangle/vertex/size stats.

Then writes review.html, which shows, per item and side by side: the picked
image that was fed in and the turntable of what came out. Judging a generated
mesh without its input next to it tells you nothing about whether the
pipeline is working or the reference was simply bad.

Triangle count is surfaced prominently because it is the decisive number: raw
TRELLIS.2 output runs into the hundreds of thousands and every held item must
come down to roughly RETOPO_TRI_TARGET through the retopo + rebake build.

Run:
  python3 gen_meshes.py                        # everything in selection.json
  python3 gen_meshes.py --only iron_hatchet
  python3 gen_meshes.py --html-only            # rebuild review.html from what exists
"""
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CANDIDATES = HERE / "candidates"
MESHES = HERE / "meshes"
SELECTION = HERE / "selection.json"
PROMPTS = HERE / "prompts.json"

MESH_WORKER = Path.home() / "Desktop/dev/mesh-worker"
GEN_MESH = MESH_WORKER / "gen_mesh.py"
# gen_mesh.py downloads the finished glb over RunPod's S3 API, which needs
# boto3. The system python (homebrew 3.14, externally managed) does not have
# it; the mesh-worker venv does. Falling back to sys.executable would submit
# fine but then fail every download.
VENV_PY = MESH_WORKER / ".venv/bin/python3"
BLENDER = "/Applications/Blender.app/Contents/MacOS/Blender"

# Only these groups get a mesh: `held` picks are icon+mesh references, and
# `generic` is the bundle viewmodel. The three icon-only groups never reach
# the worker.
MESH_GROUPS = {"generic", "held"}
# ANIMATABLE viewmodels (bow limbs/string flex, crossbow string slides,
# bandage tail unrolls) keep their authored multi-primitive glbs; a
# single-primitive TRELLIS mesh cannot carry their rig slots. Their picks
# ship as icons only until a rigging path exists.
ANIMATED_SKIP = {"wooden_bow", "crossbow", "bandage"}


def load_meta() -> dict:
    spec = json.loads(PROMPTS.read_text())
    return {i["key"]: i for i in spec["items"]}
# TRELLIS's NATIVE simplifier target: the worker exports at roughly this
# count with full silhouette fidelity (its own mesh-aware reduction + baked
# texture), and build_held.py keeps the mesh/UVs/texture as-is. This replaced
# the local voxel-remesh + dissolve + rebake recipe, which eroded thin
# features and faceted smooth surfaces.
RETOPO_TRI_TARGET = 10000


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def load_env() -> dict:
    """mesh-worker/.env holds the endpoint + S3 keys. gen_mesh.py reads them
    from the environment, so splice the file in rather than duplicating config."""
    env = dict(os.environ)
    dotenv = MESH_WORKER / ".env"
    if dotenv.is_file():
        for line in dotenv.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            k, v = line.split("=", 1)
            env.setdefault(k.strip(), v.strip().strip('"').strip("'"))
    return env


def build_mesh(key: str, ref: Path, env: dict, target: int) -> "Path | None":
    out = MESHES / f"{key}_lowpoly.glb"
    if out.exists():
        log(f"  skip (exists): {out.name}")
        return out
    log(f"  submitting {ref.name} -> {out.name} (about 9 min per mesh)")
    py = str(VENV_PY) if VENV_PY.is_file() else sys.executable
    proc = subprocess.run(
        [py, str(GEN_MESH), "--image", str(ref), "--out", str(out),
         "--decimation-target", str(target)],
        env=env,
    )
    if proc.returncode != 0 or not out.exists():
        log(f"  FAILED: {key}")
        return None
    return out


def render_turntable(glb: Path) -> dict:
    strip = MESHES / f"{glb.stem}_turntable.png"
    proc = subprocess.run(
        [BLENDER, "--background", "--python",
         str(HERE.parent / "pipeline" / "render_turntable.py"),
         "--", str(glb), str(strip), "4", "512", "color"],
        capture_output=True, text=True,
    )
    for line in proc.stdout.splitlines():
        if line.startswith("STATS_JSON "):
            stats = json.loads(line[len("STATS_JSON "):])
            stats["frames"] = [Path(f).name for f in stats.get("frames", [])]
            return stats
    log(f"  turntable failed for {glb.name}: {proc.stdout[-500:]}{proc.stderr[-500:]}")
    return {"error": "render failed"}


REVIEW_CSS = """
:root { color-scheme: dark; }
* { box-sizing: border-box; }
body { margin: 0; padding: 28px 32px 60px; background: #14161a; color: #e8e6e1;
       font: 14px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
h1 { font-size: 21px; margin: 0 0 6px; }
.lede { color: #9aa0a8; margin: 0 0 30px; max-width: 68ch; }
.item { border: 1px solid #262a31; border-radius: 12px; margin-bottom: 22px;
        background: #191c21; overflow: hidden; }
.item > header { display: flex; align-items: baseline; gap: 12px; padding: 13px 18px;
                 background: #1e222a; border-bottom: 1px solid #262a31; }
.item > header h2 { font-size: 16px; margin: 0; }
.item > header .sub { color: #868d96; font-size: 12.5px; }
.verdict { margin-left: auto; font-size: 12.5px; font-weight: 600; }
.verdict.bad { color: #d98080; }
.verdict.ok { color: #7fc98b; }
.body { display: grid; grid-template-columns: 190px 1fr; gap: 18px; padding: 18px; }
.pane h3 { font-size: 11px; text-transform: uppercase; letter-spacing: .09em;
           color: #868d96; margin: 0 0 9px; font-weight: 600; }
.frame { border: 1px solid #2b3038; border-radius: 9px; background: #101216;
         display: grid; place-items: center; padding: 10px; aspect-ratio: 1; }
.frame img { max-width: 100%; max-height: 100%; }
.strip { display: grid; grid-template-columns: repeat(4, 1fr); gap: 8px; }
.strip .frame { aspect-ratio: 1; padding: 0; overflow: hidden; }
.strip .frame img { width: 100%; height: 100%; object-fit: cover; }
.stats { margin-top: 11px; display: flex; gap: 22px; flex-wrap: wrap;
         font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; color: #9aa0a8; }
.stats b { color: #e8e6e1; font-weight: 600; }
.stats .warn b { color: #d98080; }
.missing { color: #7c838c; padding: 26px 10px; text-align: center; font-size: 12.5px; }
"""


def review_section(key: str, pick: int, stats: dict, label: str) -> str:
    ref = f"candidates/item_{key}_v{pick}.png" if pick else None
    tris = stats.get("tris")
    frames = stats.get("frames") or []

    if frames:
        strip = "".join(
            f'<div class="frame"><img src="meshes/{f}" alt="angle"></div>' for f in frames
        )
    else:
        strip = f'<div class="missing">{stats.get("error", "not generated yet")}</div>'

    if tris is None:
        verdict, cls = "", ""
    elif tris > 50 * RETOPO_TRI_TARGET:
        verdict, cls = f"{tris // 1000}k triangles, needs retopology", "bad"
    else:
        verdict, cls = f"{tris} triangles, within budget", "ok"

    stat_html = ""
    if tris is not None:
        over = tris / RETOPO_TRI_TARGET
        stat_html = (
            f'<div class="stats">'
            f'<span class="{"warn" if over > 50 else ""}">triangles <b>{tris:,}</b></span>'
            f'<span>vertices <b>{stats.get("verts", 0):,}</b></span>'
            f'<span>objects <b>{stats.get("objects", 0)}</b></span>'
            f'<span>bbox <b>{" x ".join(str(v) for v in stats.get("size_m", []))}</b></span>'
            f'<span>vs {RETOPO_TRI_TARGET} target <b>{over:.0f}x</b></span>'
            f"</div>"
        )

    def pane(title, src, alt):
        inner = (f'<img src="{src}" alt="{alt}">' if src
                 else '<span class="missing">none picked</span>')
        return (f'<div class="pane"><h3>{title}</h3>'
                f'<div class="frame">{inner}</div></div>')

    return f"""
<section class="item">
  <header>
    <h2>{label}</h2>
    <span class="sub">{key}</span>
    <span class="verdict {cls}">{verdict}</span>
  </header>
  <div class="body">
    {pane("Picked image (icon + reference)", ref, "reference")}
    <div class="pane">
      <h3>Generated mesh <em style="font-style:normal;color:#5e646c;text-transform:none;letter-spacing:0">4 angles, 90 degrees apart</em></h3>
      <div class="strip">{strip}</div>
      {stat_html}
    </div>
  </div>
</section>"""


def build_review(selection: dict, all_stats: dict, meta: dict) -> Path:
    sections = "".join(
        review_section(k, selection[k], all_stats.get(k, {}),
                       meta[k].get("label", k))
        for k in selection
        if meta[k]["group"] in MESH_GROUPS and k not in ANIMATED_SKIP
    )
    html = f"""<!doctype html>
<meta charset="utf-8">
<title>Ashwend held-item rework - mesh review</title>
<style>{REVIEW_CSS}</style>
<h1>Held-item rework, generated meshes</h1>
<p class="lede">Each row shows the reference that was fed to the image-to-3D
worker, the item icon chosen alongside it, and four angles of what came back.
Check the back and side angles, not just the front: a reconstruction that reads
well only from the reference camera is a failed one. Thin handles are the known
weak spot, so look for wavy or mushy hafts. Triangle counts come down to about
{RETOPO_TRI_TARGET} in the retopo + rebake build step.</p>
{sections}
"""
    out = HERE / "review.html"
    out.write_text(html)
    return out


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--only", help="Comma-separated item ids. An EXPLICIT list "
                                  "bypasses the mesh-group / animated-skip "
                                  "gates (used for the deployable world-mesh "
                                  "lane and the animatable integrations).")
    p.add_argument("--target", type=int, default=RETOPO_TRI_TARGET,
                   help="TRELLIS decimation target (world-placed props run "
                        "far leaner than held items)")
    p.add_argument("--html-only", action="store_true")
    args = p.parse_args()

    if not SELECTION.is_file():
        log(f"missing {SELECTION}. Pick candidates in pick.html, then use "
            "'Download selection.json' (or paste the copied blob into that file).")
        sys.exit(1)
    selection = json.loads(SELECTION.read_text())
    meta = load_meta()
    MESHES.mkdir(parents=True, exist_ok=True)

    stats_path = MESHES / "stats.json"
    all_stats = json.loads(stats_path.read_text()) if stats_path.is_file() else {}

    if not args.html_only:
        env = load_env()
        explicit = set(args.only.split(",")) if args.only else None
        for key, pick in selection.items():
            if explicit is not None and key not in explicit:
                continue
            if not pick:
                log(f"{key}: nothing picked, skipping mesh")
                continue
            # The group/animated gates guard the DEFAULT everything run; an
            # explicit --only list is a deliberate lane (deployable world
            # meshes, animatable integrations) and goes through.
            if explicit is None:
                if meta[key]["group"] not in MESH_GROUPS:
                    continue
                if key in ANIMATED_SKIP:
                    log(f"{key}: animatable viewmodel, icon-only this round")
                    continue
            log(f"\n=== {key} ===")
            ref = CANDIDATES / f"item_{key}_v{pick}.png"
            if not ref.is_file():
                log(f"  missing reference {ref.name}")
                continue
            glb = build_mesh(key, ref, env, args.target)
            if glb:
                all_stats[key] = render_turntable(glb)
                stats_path.write_text(json.dumps(all_stats, indent=2) + "\n")

    out = build_review(selection, all_stats, meta)
    log(f"\nReview: {out}")
    log(f"Open with:  open {out}")


if __name__ == "__main__":
    main()
