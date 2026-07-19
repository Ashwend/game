#!/usr/bin/env python3
"""Stage 2: take the picker's selection, build meshes, write the review page.

Reads selection.json (the blob copied out of pick.html), and for each node type:
  1. submits the CHOSEN deposit reference to the RunPod TRELLIS.2 worker,
  2. renders a 4-angle turntable of the resulting .glb in Blender,
  3. collects triangle/vertex/size stats.

Then writes review.html, which shows, per node type and side by side: the
reference image that was fed in, the item icon that was chosen alongside it, and
the turntable of what came out. That three-way comparison is the point. Judging
a generated mesh without its input next to it tells you nothing about whether
the pipeline is working or the reference was simply bad.

Triangle count is surfaced prominently because it is the decisive number. The
authored ore glbs are a few hundred triangles; raw TRELLIS.2 output measured
954,705 on the barrel test and could not be decimated without shattering.

Run:
  python3 gen_meshes.py                    # everything in selection.json
  python3 gen_meshes.py --only iron
  python3 gen_meshes.py --html-only        # rebuild review.html from what exists
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
# fine but then fail every download, which is exactly what burned the first
# batch: five jobs completed on the volume and zero arrived locally.
VENV_PY = MESH_WORKER / ".venv/bin/python3"
BLENDER = "/Applications/Blender.app/Contents/MacOS/Blender"

NODE_LABELS = {
    "stone": "Stone Vein", "iron": "Iron Node", "coal": "Coal Node",
    "sulfur": "Sulfur Node", "meteorite": "Meteorite",
}
ITEM_LABELS = {
    "stone": "Stone", "iron": "Iron Ore", "coal": "Coal",
    "sulfur": "Sulfur Ore", "meteorite": "Meteorite Alloy",
}
# What the authored, hand-built ore glbs cost, for scale against the generated
# result. Source: art/ore/build_ore.py output.
AUTHORED_TRI_BUDGET = 400


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


def build_mesh(key: str, ref: Path, env: dict) -> "Path | None":
    out = MESHES / f"{key}.glb"
    if out.exists():
        log(f"  skip (exists): {out.name}")
        return out
    log(f"  submitting {ref.name} -> {out.name} (about 9 min per mesh)")
    py = str(VENV_PY) if VENV_PY.is_file() else sys.executable
    proc = subprocess.run(
        [py, str(GEN_MESH), "--image", str(ref), "--out", str(out)],
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
         "--", str(glb), str(strip), "4", "512"],
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
.node { border: 1px solid #262a31; border-radius: 12px; margin-bottom: 22px;
        background: #191c21; overflow: hidden; }
.node > header { display: flex; align-items: baseline; gap: 12px; padding: 13px 18px;
                 background: #1e222a; border-bottom: 1px solid #262a31; }
.node > header h2 { font-size: 16px; margin: 0; }
.node > header .sub { color: #868d96; font-size: 12.5px; }
.verdict { margin-left: auto; font-size: 12.5px; font-weight: 600; }
.verdict.bad { color: #d98080; }
.verdict.ok { color: #7fc98b; }
.body { display: grid; grid-template-columns: 190px 190px 1fr; gap: 18px; padding: 18px; }
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


def review_section(key: str, pick: dict, stats: dict) -> str:
    ref = f"candidates/node_{key}_v{pick['node']}.png" if pick.get("node") else None
    icon = f"candidates/icon_{key}_v{pick['icon']}.png" if pick.get("icon") else None
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
    elif tris > 50 * AUTHORED_TRI_BUDGET:
        verdict, cls = f"{tris // 1000}k triangles, needs retopology", "bad"
    else:
        verdict, cls = f"{tris} triangles, within budget", "ok"

    stat_html = ""
    if tris is not None:
        over = tris / AUTHORED_TRI_BUDGET
        stat_html = (
            f'<div class="stats">'
            f'<span class="{"warn" if over > 50 else ""}">triangles <b>{tris:,}</b></span>'
            f'<span>vertices <b>{stats.get("verts", 0):,}</b></span>'
            f'<span>objects <b>{stats.get("objects", 0)}</b></span>'
            f'<span>bbox <b>{" x ".join(str(v) for v in stats.get("size_m", []))}</b></span>'
            f'<span>vs authored <b>{over:.0f}x</b></span>'
            f"</div>"
        )

    def pane(title, src, alt):
        inner = (f'<img src="{src}" alt="{alt}">' if src
                 else '<span class="missing">none picked</span>')
        return (f'<div class="pane"><h3>{title}</h3>'
                f'<div class="frame">{inner}</div></div>')

    return f"""
<section class="node">
  <header>
    <h2>{NODE_LABELS[key]}</h2>
    <span class="sub">yields {ITEM_LABELS[key]}</span>
    <span class="verdict {cls}">{verdict}</span>
  </header>
  <div class="body">
    {pane("Reference fed in", ref, "reference")}
    {pane("Item icon", icon, "icon")}
    <div class="pane">
      <h3>Generated mesh <em style="font-style:normal;color:#5e646c;text-transform:none;letter-spacing:0">4 angles, 90 degrees apart</em></h3>
      <div class="strip">{strip}</div>
      {stat_html}
    </div>
  </div>
</section>"""


def build_review(selection: dict, all_stats: dict) -> Path:
    sections = "".join(
        review_section(k, selection[k], all_stats.get(k, {}))
        for k in selection
    )
    html = f"""<!doctype html>
<meta charset="utf-8">
<title>Ashwend node rework - mesh review</title>
<style>{REVIEW_CSS}</style>
<h1>Resource node rework, generated meshes</h1>
<p class="lede">Each row shows the reference that was fed to the image-to-3D
worker, the item icon chosen alongside it, and four angles of what came back.
Check the back and side angles, not just the front: a reconstruction that reads
well only from the reference camera is a failed one. Triangle count is measured
against the authored ore glbs at roughly {AUTHORED_TRI_BUDGET} triangles.</p>
{sections}
"""
    out = HERE / "review.html"
    out.write_text(html)
    return out


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--only", help="Only this node type")
    p.add_argument("--html-only", action="store_true")
    args = p.parse_args()

    if not SELECTION.is_file():
        log(f"missing {SELECTION}. Pick candidates in pick.html, then use "
            "'Download selection.json' (or paste the copied blob into that file).")
        sys.exit(1)
    selection = json.loads(SELECTION.read_text())
    MESHES.mkdir(parents=True, exist_ok=True)

    stats_path = MESHES / "stats.json"
    all_stats = json.loads(stats_path.read_text()) if stats_path.is_file() else {}

    if not args.html_only:
        env = load_env()
        for key, pick in selection.items():
            if args.only and key != args.only:
                continue
            if not pick.get("node"):
                log(f"{key}: no deposit picked, skipping mesh")
                continue
            log(f"\n=== {key} ===")
            ref = CANDIDATES / f"node_{key}_v{pick['node']}.png"
            if not ref.is_file():
                log(f"  missing reference {ref.name}")
                continue
            glb = build_mesh(key, ref, env)
            if glb:
                all_stats[key] = render_turntable(glb)
                stats_path.write_text(json.dumps(all_stats, indent=2) + "\n")

    out = build_review(selection, all_stats)
    log(f"\nReview: {out}")
    log(f"Open with:  open {out}")


if __name__ == "__main__":
    main()
