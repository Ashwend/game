#!/usr/bin/env python3
"""Generate the item-art candidate set (unified icon + mesh reference).

Batch 2 covers every remaining item, grouped by what the pick becomes:
  generic          the burlap bundle replacing the procedural bag viewmodel
                   (mesh-bound, no icon ships from it)
  held             icon AND image-to-3D mesh reference (like the five tools)
  deployable_icon  icon only (placed world model already exists)
  material_icon    icon only (never reaches a hand)
  armor_icon       icon only (worn rig model already exists)

Schema (prompts.json): three backbones (default / glow / cluster) picked per
item, an item-level subject shared by the four variants (a variant may still
override it), and a per-item orient string injected into the backbone.

Everything routes through the skill's generate.py so the cutout + trim +
recenter post-processing is identical to every other Ashwend asset; the
RunPod 2D lane is the default backend (ASHWEND_RENDER_BACKEND in the
environment wins).

Run:
  python3 gen_candidates.py                        # everything + pick.html
  python3 gen_candidates.py --only torch,arrow     # a comma-separated subset
  python3 gen_candidates.py --shard 0/3            # every 3rd item (parallel lanes)
  python3 gen_candidates.py --html-only            # rebuild pick.html only
"""
import argparse
import json
import os
import subprocess
import sys
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
CANDIDATES = HERE / "candidates"
PROMPTS = HERE / "prompts.json"
ASSETS_ITEMS = HERE.parent.parent / "assets" / "items"
GENERATE = Path.home() / ".claude/skills/lowpoly-game-assets/scripts/generate.py"

SIZE, MARGIN = 1024, 0.06


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def compose(spec: dict, item: dict, variant: dict) -> str:
    """Backbone (per item) + subject (variant overrides item) + orient + extra."""
    backbone = spec["backbones"][item.get("backbone", "default")]
    subject = variant.get("subject") or item["subject"]
    orient = item["orient"]
    prompt = backbone.format(subject=subject, orient=orient)
    extra = (variant.get("extra") or "").strip()
    return f"{prompt}, {extra}" if extra else prompt


def seed_base(key: str) -> int:
    """Deterministic across runs (Python's str hash is salted per process)."""
    return zlib.crc32(key.encode()) % 100000


def generate_one(prompt: str, out: Path, seed: int) -> bool:
    """One image via the skill's icon path (rembg cutout + trim + recenter)."""
    if out.exists():
        log(f"  skip (exists): {out.name}")
        return True
    env = dict(os.environ)
    env.setdefault("ASHWEND_RENDER_BACKEND", "runpod")
    cmd = [
        sys.executable, str(GENERATE), "icon",
        "--prompt", prompt,
        "--out", str(out),
        "--size", str(SIZE),
        "--margin", str(MARGIN),
        "--variants", "1",
        "--seed", str(seed),
    ]
    proc = subprocess.run(cmd, env=env)
    if proc.returncode != 0 or not out.exists():
        log(f"  FAILED: {out.name}")
        return False
    log(f"  -> {out.name}")
    return True


def select_items(spec: dict, only: "str | None", shard: "str | None") -> list:
    items = spec["items"]
    if only:
        wanted = {k.strip() for k in only.split(",") if k.strip()}
        unknown = wanted - {i["key"] for i in items}
        if unknown:
            log(f"unknown item ids: {sorted(unknown)}")
            sys.exit(1)
        items = [i for i in items if i["key"] in wanted]
    if shard:
        k, n = (int(x) for x in shard.split("/"))
        items = [item for idx, item in enumerate(items) if idx % n == k]
    return items


def run_generation(spec: dict, items: list, seed_bump: int) -> None:
    CANDIDATES.mkdir(parents=True, exist_ok=True)
    for item in items:
        key = item["key"]
        log(f"\n=== {key} ===")
        base = seed_base(key)
        for i, variant in enumerate(item["prompts"], start=1):
            out = CANDIDATES / f"item_{key}_v{i}.png"
            # Per-key seed lane (base + 1000 + i). `--seed-bump N` shifts the
            # lane for RE-ROLLS: delete a dud file first, then rerun with a
            # bump; surviving files skip, so only the deleted slot
            # regenerates, with a genuinely new seed.
            generate_one(compose(spec, item, variant), out,
                         base + 1000 + i + seed_bump * 7919)


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
.group-head { margin: 40px 0 14px; }
.group-head h2 { font-size: 17px; margin: 0 0 4px; color: #d8b98a; }
.group-head p { margin: 0; color: #9aa0a8; font-size: 13px; max-width: 74ch; }
.item { border: 1px solid #262a31; border-radius: 12px; margin-bottom: 26px;
        background: #191c21; overflow: hidden; }
.item > header { display: flex; align-items: baseline; gap: 12px;
                 padding: 13px 18px; background: #1e222a; border-bottom: 1px solid #262a31; }
.item > header h2 { font-size: 16px; margin: 0; }
.item > header .sub { color: #868d96; font-size: 12.5px; }
.item > header .status { margin-left: auto; font-size: 12.5px; color: #f0a868; }
.item > header .status.done { color: #7fc98b; }
.band { padding: 16px 18px 20px; }
.row { display: flex; gap: 14px; align-items: flex-start; }
.current { flex: 0 0 122px; }
.current .frame { height: 122px; border: 1px dashed #343941; border-radius: 9px;
                  background: #101216; display: grid; place-items: center; padding: 8px; }
.current img { max-width: 100%; max-height: 100%; }
.current .none { color: #565c64; font-size: 11px; text-align: center; }
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

document.querySelectorAll('.card').forEach(card => {
  if (card.classList.contains('is-missing')) return;
  card.addEventListener('click', () => {
    const t = card.dataset.type, v = +card.dataset.variant;
    if (sel[t] === v) {
      delete sel[t];                                  // click again to unpick
    } else {
      sel[t] = v;
    }
    document.querySelectorAll(
      `.card[data-type="${t}"]`
    ).forEach(c => c.classList.toggle('sel', +c.dataset.variant === sel[t]));
    refresh();
  });
});

function refresh() {
  let complete = 0;
  TYPES.forEach(t => {
    const done = !!sel[t];
    if (done) complete++;
    const badge = document.querySelector(`.status[data-type="${t}"]`);
    badge.textContent = done ? 'ready' : 'nothing picked';
    badge.className = done ? 'status done' : 'status';
  });
  document.getElementById('n').textContent = complete;
  const json = JSON.stringify(sel);
  document.getElementById('out').textContent =
    complete ? json : 'pick one image per item (partial selections are fine, unpicked items keep shipping art)';
  document.getElementById('copy').disabled = complete === 0;
  document.getElementById('save').disabled = complete === 0;
}

document.getElementById('copy').addEventListener('click', async () => {
  await navigator.clipboard.writeText(JSON.stringify(sel));
  const b = document.getElementById('copy');
  b.textContent = 'copied';
  setTimeout(() => (b.textContent = 'Copy selection'), 1400);
});

document.getElementById('save').addEventListener('click', () => {
  const blob = new Blob([JSON.stringify(sel, null, 2)], { type: 'application/json' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = 'selection.json';
  a.click();
});

refresh();
"""


def card_html(key: str, index: int, variant: dict) -> str:
    name = f"item_{key}_v{index}.png"
    exists = (CANDIDATES / name).exists()
    label = variant.get("label", f"variant {index}")
    body = (
        f'<div class="shot"><img src="candidates/{name}" alt="{label}"></div>'
        if exists else
        '<div class="missing">not generated</div>'
    )
    missing_cls = "" if exists else " is-missing"
    return (
        f'<div class="card{missing_cls}" data-type="{key}" '
        f'data-variant="{index}">{body}'
        f'<div class="meta"><div class="name"><span class="n">{index}</span>{label}</div></div>'
        f"</div>"
    )


def item_html(item: dict) -> str:
    key = item["key"]
    cards = "".join(
        card_html(key, i, v) for i, v in enumerate(item["prompts"], start=1)
    )
    icon = ASSETS_ITEMS / key / "icon.png"
    current = (
        f'<img src="../../assets/items/{key}/icon.png" alt="current">'
        if icon.is_file() else '<div class="none">no shipping art yet</div>'
    )
    return f"""
<section class="item">
  <header>
    <h2>{item.get("label", key)}</h2>
    <span class="sub">{key}</span>
    <span class="status" data-type="{key}">nothing picked</span>
  </header>
  <div class="band">
    <div class="row">
      <div class="current">
        <div class="frame">{current}</div>
        <div class="cap">shipping now</div>
      </div>
      <div class="grid">{cards}</div>
    </div>
  </div>
</section>"""


def build_picker(spec: dict, only_keys: "set | None" = None,
                 out_name: str = "pick.html") -> Path:
    items = [i for i in spec["items"]
             if only_keys is None or i["key"] in only_keys]
    keys = [n["key"] for n in items]
    groups = spec["groups"]
    sections = []
    for group_key in spec["group_order"]:
        members = [i for i in items if i["group"] == group_key]
        if not members:
            continue
        meta = groups[group_key]
        sections.append(
            f'<div class="group-head"><h2>{meta["title"]}</h2>'
            f'<p>{meta["note"]}</p></div>'
        )
        sections.extend(item_html(item) for item in members)

    html = f"""<!doctype html>
<meta charset="utf-8">
<title>Ashwend item art - batch 2 candidates</title>
<style>{PICK_CSS}</style>
<h1>Item art batch 2: everything except the five shipped tools</h1>
<p class="lede">Pick ONE image per item. In the two mesh-bound groups the pick
is also the image-to-3D reference, so glance at whether the silhouette holds
up as a 3D prop; in the icon-only groups judge it purely as an icon. Click a
picked card again to clear it. A partial selection is fine, unpicked items
keep their shipping art.</p>
{''.join(sections)}
<footer>
  <span class="count"><b id="n">0</b> / {len(keys)} items picked</span>
  <code id="out"></code>
  <button id="save">Download selection.json</button>
  <button id="copy" class="primary">Copy selection</button>
</footer>
<script>const TYPES = {json.dumps(keys)};</script>
<script>{PICK_JS}</script>
"""
    out = HERE / out_name
    out.write_text(html)
    return out


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--only", help="Comma-separated item ids to generate")
    p.add_argument("--shard", help="k/n: process every n-th item starting at k "
                                  "(parallel lanes; shards skip the picker build)")
    p.add_argument("--seed-bump", type=int, default=0,
                   help="Shift the seed lane for rerolling deleted duds")
    p.add_argument("--html-only", action="store_true",
                   help="Skip generation, just rebuild pick.html")
    p.add_argument("--picker-out", default="pick.html",
                   help="Picker file name (a dedicated redo round gets its own)")
    args = p.parse_args()

    if not PROMPTS.is_file():
        log(f"missing {PROMPTS}")
        sys.exit(1)
    spec = json.loads(PROMPTS.read_text())

    if not args.html_only:
        items = select_items(spec, args.only, args.shard)
        log(f"{len(items)} item(s), {sum(len(i['prompts']) for i in items)} images")
        run_generation(spec, items, args.seed_bump)

    if args.shard and not args.html_only:
        log("shard done (picker build skipped; run --html-only after all shards)")
        return

    only_keys = (
        {k.strip() for k in args.only.split(",")} if args.only else None
    )
    out = build_picker(spec, only_keys, args.picker_out)
    log(f"\nPicker: {out}")
    log(f"Open with:  open {out}")


if __name__ == "__main__":
    main()
