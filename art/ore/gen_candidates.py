#!/usr/bin/env python3
"""Generate the resource-node art rework candidate set, then build the picker.

Two products per node type (stone, iron, coal, sulfur, meteorite):
  * 4 DEPOSIT references, 1024 px, RGBA cutouts. These are the images fed to
    the TRELLIS.2 image-to-3D worker, so they must be background-removed
    (the worker refuses a fully opaque image: TRELLIS would otherwise fall back
    to the gated, non-commercial briaai/RMBG-2.0).
  * 4 ITEM icons, 512 px, RGBA cutouts, matching the existing inventory grid.

Everything routes through the skill's generate.py so the cutout + trim +
recenter post-processing is identical to every other Ashwend asset. The prompt
STYLE BACKBONE is supplied by prompts.json and injected via --prompt (full
override), because deposits and icons need different backbones and generate.py
only ships the icon one.

Run:
  python3 gen_candidates.py                 # generate everything + write pick.html
  python3 gen_candidates.py --only stone    # one node type
  python3 gen_candidates.py --html-only     # rebuild pick.html from what exists
"""
import argparse
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CANDIDATES = HERE / "candidates"
PROMPTS = HERE / "prompts.json"
GENERATE = Path.home() / ".claude/skills/lowpoly-game-assets/scripts/generate.py"

# Deposit refs are generated larger and tighter-cropped than icons: they feed
# image-to-3D reconstruction, where every pixel of the subject is signal and
# empty margin is waste.
NODE_SIZE, NODE_MARGIN = 1024, 0.06
ICON_SIZE, ICON_MARGIN = 512, 0.14

NODE_LABELS = {
    "stone": "Stone Vein",
    "iron": "Iron Node",
    "coal": "Coal Node",
    "sulfur": "Sulfur Node",
    "meteorite": "Meteorite",
}
ITEM_LABELS = {
    "stone": "Stone",
    "iron": "Iron Ore",
    "coal": "Coal",
    "sulfur": "Sulfur Ore",
    "meteorite": "Meteorite Alloy",
}
# The icon each node type currently ships, shown in the picker as the
# before-shot so a candidate is judged against what it would replace.
CURRENT_ICON = {
    "stone": "stone",
    "iron": "iron_ore",
    "coal": "coal",
    "sulfur": "sulfur_ore",
    "meteorite": "meteorite_alloy",
}


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def compose(backbone: str, variant: dict) -> str:
    """backbone.format(subject=...) plus the variant's own art direction."""
    prompt = backbone.format(subject=variant["subject"])
    extra = (variant.get("extra") or "").strip()
    return f"{prompt}, {extra}" if extra else prompt


def generate_one(prompt: str, out: Path, size: int, margin: float, seed: int) -> bool:
    """One image via the skill's icon path (rembg cutout + trim + recenter)."""
    if out.exists():
        log(f"  skip (exists): {out.name}")
        return True
    cmd = [
        sys.executable, str(GENERATE), "icon",
        "--prompt", prompt,
        "--out", str(out),
        "--size", str(size),
        "--margin", str(margin),
        "--variants", "1",
        "--seed", str(seed),
    ]
    proc = subprocess.run(cmd)
    if proc.returncode != 0 or not out.exists():
        log(f"  FAILED: {out.name}")
        return False
    log(f"  -> {out.name}")
    return True


def run_generation(spec: dict, only: "str | None") -> None:
    CANDIDATES.mkdir(parents=True, exist_ok=True)
    node_backbone = spec["node_backbone"]
    icon_backbone = spec["icon_backbone"]

    for node in spec["nodes"]:
        key = node["key"]
        if only and key != only:
            continue
        log(f"\n=== {key} ===")
        # Deterministic per (type, kind, variant) seeds so a re-run reproduces
        # the same candidate and a single reroll can be targeted by deleting
        # one file rather than the whole set.
        base = abs(hash(key)) % 100000
        for i, variant in enumerate(node["node_prompts"], start=1):
            out = CANDIDATES / f"node_{key}_v{i}.png"
            generate_one(compose(node_backbone, variant), out,
                         NODE_SIZE, NODE_MARGIN, base + i)
        for i, variant in enumerate(node["icon_prompts"], start=1):
            out = CANDIDATES / f"icon_{key}_v{i}.png"
            generate_one(compose(icon_backbone, variant), out,
                         ICON_SIZE, ICON_MARGIN, base + 500 + i)


# --------------------------------------------------------------------------- #
# Picker
# --------------------------------------------------------------------------- #

PICK_CSS = """
:root { color-scheme: dark; }
* { box-sizing: border-box; }
body { margin: 0; padding: 28px 32px 140px; background: #14161a; color: #e8e6e1;
       font: 14px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
h1 { font-size: 21px; margin: 0 0 6px; letter-spacing: .2px; }
.lede { color: #9aa0a8; margin: 0 0 30px; max-width: 62ch; }
.node { border: 1px solid #262a31; border-radius: 12px; margin-bottom: 26px;
        background: #191c21; overflow: hidden; }
.node > header { display: flex; align-items: baseline; gap: 12px;
                 padding: 13px 18px; background: #1e222a; border-bottom: 1px solid #262a31; }
.node > header h2 { font-size: 16px; margin: 0; }
.node > header .sub { color: #868d96; font-size: 12.5px; }
.node > header .status { margin-left: auto; font-size: 12.5px; color: #f0a868; }
.node > header .status.done { color: #7fc98b; }
.band { padding: 16px 18px 20px; }
.band + .band { border-top: 1px solid #23262d; }
.band h3 { font-size: 12px; text-transform: uppercase; letter-spacing: .09em;
           color: #868d96; margin: 0 0 13px; font-weight: 600; }
.band h3 em { font-style: normal; color: #5e646c; text-transform: none;
              letter-spacing: 0; font-weight: 400; }
.row { display: flex; gap: 14px; align-items: flex-start; }
.current { flex: 0 0 122px; }
.current .frame { height: 122px; border: 1px dashed #343941; border-radius: 9px;
                  background: #101216; display: grid; place-items: center; padding: 8px; }
.current img { max-width: 100%; max-height: 100%; }
.current .cap { font-size: 11px; color: #6b727a; text-align: center; margin-top: 6px; }
.grid { flex: 1; display: grid; grid-template-columns: repeat(4, 1fr); gap: 14px; }
.card { border: 2px solid #2b3038; border-radius: 10px; background: #101216;
        cursor: pointer; overflow: hidden; transition: border-color .12s, transform .12s; }
.card:hover { border-color: #4a525d; transform: translateY(-2px); }
.card.sel { border-color: #6fae7a; background: #131a15; }
.card .shot { aspect-ratio: 1; display: grid; place-items: center; padding: 9px;
              background-color: #101216;
              background-image: linear-gradient(45deg, #191c21 25%, transparent 25%),
                                linear-gradient(-45deg, #191c21 25%, transparent 25%),
                                linear-gradient(45deg, transparent 75%, #191c21 75%),
                                linear-gradient(-45deg, transparent 75%, #191c21 75%);
              background-size: 16px 16px;
              background-position: 0 0, 0 8px, 8px -8px, -8px 0; }
.card .shot img { max-width: 100%; max-height: 100%; }
.card .meta { padding: 7px 9px 9px; border-top: 1px solid #23262d; }
.card .name { font-size: 12px; font-weight: 600; }
.card .name .n { color: #6b727a; font-weight: 400; margin-right: 5px; }
.card.sel .name { color: #9fdcaa; }
.card .missing { color: #c96f6f; font-size: 12px; padding: 30px 8px; text-align: center; }
footer { position: fixed; left: 0; right: 0; bottom: 0; background: #1b1f26;
         border-top: 1px solid #2b3038; padding: 14px 32px;
         display: flex; align-items: center; gap: 16px; }
footer .count { color: #9aa0a8; }
footer .count b { color: #e8e6e1; }
button { font: inherit; font-weight: 600; padding: 9px 17px; border-radius: 8px;
         border: 1px solid #3a414b; background: #262b33; color: #e8e6e1; cursor: pointer; }
button:hover { background: #2f353f; }
button.primary { background: #3f7a4a; border-color: #4b8f57; }
button.primary:hover { background: #478953; }
button:disabled { opacity: .4; cursor: not-allowed; }
#out { flex: 1; font: 12px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace;
       background: #101216; border: 1px solid #2b3038; border-radius: 7px;
       padding: 9px 11px; color: #8fbf9a; white-space: pre; overflow-x: auto; }
"""

PICK_JS = """
const sel = {};
function key(t, kind) { return t + '|' + kind; }

document.querySelectorAll('.card').forEach(card => {
  if (card.classList.contains('is-missing')) return;
  card.addEventListener('click', () => {
    const t = card.dataset.type, kind = card.dataset.kind, v = +card.dataset.variant;
    if (sel[key(t, kind)] === v) {
      delete sel[key(t, kind)];                       // click again to unpick
    } else {
      sel[key(t, kind)] = v;
    }
    document.querySelectorAll(
      `.card[data-type="${t}"][data-kind="${kind}"]`
    ).forEach(c => c.classList.toggle('sel', +c.dataset.variant === sel[key(t, kind)]));
    refresh();
  });
});

function payload() {
  const out = {};
  TYPES.forEach(t => {
    const n = sel[key(t, 'node')], i = sel[key(t, 'icon')];
    if (n || i) out[t] = { node: n || null, icon: i || null };
  });
  return out;
}

function refresh() {
  const p = payload();
  let complete = 0;
  TYPES.forEach(t => {
    const row = p[t], done = row && row.node && row.icon;
    if (done) complete++;
    const badge = document.querySelector(`.status[data-type="${t}"]`);
    if (!row) { badge.textContent = 'nothing picked'; badge.className = 'status'; }
    else if (done) { badge.textContent = 'ready'; badge.className = 'status done'; }
    else { badge.textContent = row.node ? 'needs an item icon' : 'needs a deposit'; badge.className = 'status'; }
  });
  document.getElementById('n').textContent = complete;
  const json = JSON.stringify(p);
  document.getElementById('out').textContent = complete ? json : 'pick a deposit and an item icon for each node type';
  document.getElementById('copy').disabled = complete === 0;
  document.getElementById('save').disabled = complete === 0;
}

document.getElementById('copy').addEventListener('click', async () => {
  await navigator.clipboard.writeText(JSON.stringify(payload()));
  const b = document.getElementById('copy');
  b.textContent = 'copied';
  setTimeout(() => (b.textContent = 'Copy selection'), 1400);
});

document.getElementById('save').addEventListener('click', () => {
  const blob = new Blob([JSON.stringify(payload(), null, 2)], { type: 'application/json' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = 'selection.json';
  a.click();
});

refresh();
"""


def card_html(kind: str, key: str, index: int, variant: dict, rel: Path) -> str:
    exists = (CANDIDATES / rel.name).exists()
    label = variant.get("label", f"variant {index}")
    body = (
        f'<div class="shot"><img src="candidates/{rel.name}" alt="{label}"></div>'
        if exists else
        '<div class="missing">not generated</div>'
    )
    missing_cls = "" if exists else " is-missing"
    return (
        f'<div class="card{missing_cls}" data-type="{key}" data-kind="{kind}" '
        f'data-variant="{index}">{body}'
        f'<div class="meta"><div class="name"><span class="n">{index}</span>{label}</div></div>'
        f"</div>"
    )


def build_picker(spec: dict) -> Path:
    keys = [n["key"] for n in spec["nodes"]]
    sections = []
    for node in spec["nodes"]:
        key = node["key"]
        node_cards = "".join(
            card_html("node", key, i, v, Path(f"node_{key}_v{i}.png"))
            for i, v in enumerate(node["node_prompts"], start=1)
        )
        icon_cards = "".join(
            card_html("icon", key, i, v, Path(f"icon_{key}_v{i}.png"))
            for i, v in enumerate(node["icon_prompts"], start=1)
        )
        current = CURRENT_ICON[key]
        sections.append(f"""
<section class="node">
  <header>
    <h2>{NODE_LABELS[key]}</h2>
    <span class="sub">yields {ITEM_LABELS[key]}</span>
    <span class="status" data-type="{key}">nothing picked</span>
  </header>
  <div class="band">
    <h3>Deposit <em>the boulder in the world, this becomes the 3D mesh</em></h3>
    <div class="row"><div class="grid">{node_cards}</div></div>
  </div>
  <div class="band">
    <h3>Item icon <em>{ITEM_LABELS[key]} in the inventory grid</em></h3>
    <div class="row">
      <div class="current">
        <div class="frame"><img src="../../../assets/items/{current}/icon.png" alt="current"></div>
        <div class="cap">shipping now</div>
      </div>
      <div class="grid">{icon_cards}</div>
    </div>
  </div>
</section>""")

    html = f"""<!doctype html>
<meta charset="utf-8">
<title>Ashwend node art rework - pick candidates</title>
<style>{PICK_CSS}</style>
<h1>Resource node rework</h1>
<p class="lede">Pick one deposit and one item icon per node type. The deposit you
choose is the image fed to the image-to-3D worker, so judge it on silhouette and
on how crisply faceted it reads, not on its texture detail. Click a picked card
again to clear it.</p>
{''.join(sections)}
<footer>
  <span class="count"><b id="n">0</b> / {len(keys)} node types ready</span>
  <code id="out"></code>
  <button id="save">Download selection.json</button>
  <button id="copy" class="primary">Copy selection</button>
</footer>
<script>const TYPES = {json.dumps(keys)};</script>
<script>{PICK_JS}</script>
"""
    out = HERE / "pick.html"
    out.write_text(html)
    return out


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--only", help="Generate only this node type")
    p.add_argument("--html-only", action="store_true",
                   help="Skip generation, just rebuild pick.html")
    args = p.parse_args()

    if not PROMPTS.is_file():
        log(f"missing {PROMPTS}")
        sys.exit(1)
    spec = json.loads(PROMPTS.read_text())

    if not args.html_only:
        run_generation(spec, args.only)

    out = build_picker(spec)
    log(f"\nPicker: {out}")
    log(f"Open with:  open {out}")


if __name__ == "__main__":
    main()
