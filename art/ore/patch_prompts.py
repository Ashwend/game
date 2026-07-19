#!/usr/bin/env python3
"""Apply the pilot-batch corrections to prompts.json.

Kept as a script rather than hand-edits so the reasoning survives and a
re-derived prompt set can be re-patched. Each fix below is a measured response
to what the stone pilot actually rendered, not a guess.

Findings from the stone pilot (8 images):

1. COLOUR, the big one. "Bright warm-grey rock body" rendered as SANDSTONE TAN
   on all four deposits. FLUX weighted "warm" far above "grey". The target is
   art/ore/build_ore.py's _ROCK = (0.430, 0.400, 0.360) linear, which is a
   near-neutral grey with only a whisper of warmth, nowhere near tan. Worse, the
   icons came back correctly grey, so deposits and icons disagreed about the
   colour of the same material. Fix: drop "warm" next to "grey" everywhere and
   ban the failure colours explicitly in both backbones.

2. VALUE SPREAD. The four stone icons ran from near-white (v2) to dark charcoal
   (v1). Inconsistency is the whole reason for this rework, so the icon backbone
   now pins a mid-grey value anchor.

3. SCALE. The "craggy tilted upthrust" variant rendered as a tall monolith,
   reading like a cliff rather than a knee-high deposit. "Slightly taller than
   wide" was taken as licence to go vertical. Fix: restate it as a low, squat,
   wider-than-tall deposit that merely leans.

4. TYPE CONFUSION. Stone vein's "darker charcoal-grey knobs" rendered as dark
   slate slabs that read as COAL, which would collide with the actual coal node
   in-world. The brief says the stone vein has no bright mineral chunks and its
   "chunks" are just exposed rock. Fix: same stone, one shade down, no charcoal.

Faceting is deliberately NOT touched: the large-flat-plane instruction is what
finally produced retopologisable geometry, and it worked on the first try.
"""
import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
PROMPTS = HERE / "prompts.json"

# Appended to BOTH backbones. Stated positively plus explicit exclusions,
# because FLUX schnell is guidance-distilled and ignores negative prompts.
COLOUR_LOCK = (
    ", neutral stone grey the colour of bare granite, no tan, no beige, "
    "no sandstone, no orange or brown tint, medium value that is neither "
    "near-white nor near-black"
)

# Applied to every "subject" clause. THIS is where the tan actually came from:
# the first patch fixed only the extras, and the deposits stayed sandstone while
# the icons came out correctly grey. The difference was that every deposit
# SUBJECT still read "warm-grey stone boulder" while no icon subject contained
# the word "warm". The subject leads the prompt and dominates FLUX's attention,
# so a correction buried 60 words later in the extra never wins against it.
# Lesson worth keeping: patch the subject first, the extra second.
SUBJECT_FIXES = [
    ("warm-grey stone", "grey granite"),
    ("warm-grey boulder", "grey granite boulder"),
    ("warm-grey", "grey"),
]

# Whole-string replacements applied to every "extra" clause.
EXTRA_FIXES = [
    # 1. Kill the tan.
    ("Bright warm-grey rock body.",
     "Neutral mid-grey granite body, no tan or beige."),
    ("Neutral warm grey with almost no colour.",
     "Neutral mid-grey granite, no tan or beige."),
    ("Neutral warm grey, almost no colour,",
     "Neutral mid-grey granite, no tan or beige,"),
    ("Muted grey stone", "Neutral mid-grey granite"),
    ("muted grey stone", "neutral mid-grey granite"),
    ("a muted grey stone rind", "a neutral mid-grey granite rind"),
    ("weathered muted grey stone rind", "weathered neutral mid-grey granite rind"),
    # 3. Bring the upthrust back down to deposit scale.
    ("Leaning upthrust, slightly taller than wide but still knee high,",
     "Low leaning deposit, knee high and no taller than it is wide,"),
    ("Leaning craggy mass rising to an off-centre peak, still low and stout "
     "rather than tall,",
     "Low leaning mass with an off-centre peak, knee high and no taller than "
     "it is wide,"),
]

# 4. Stone-vein only: its knobs must not read as coal.
STONE_ONLY_FIXES = [
    ("darker charcoal-grey knobs", "slightly darker grey rock knobs of the same granite"),
    ("darker charcoal-grey knobs clustered", "slightly darker grey rock knobs clustered"),
]


def fix_extra(text: str, fixes) -> str:
    for old, new in fixes:
        text = text.replace(old, new)
    return text


def main() -> None:
    spec = json.loads(PROMPTS.read_text())

    for field in ("node_backbone", "icon_backbone"):
        if COLOUR_LOCK.strip(", ") not in spec[field]:
            spec[field] = spec[field] + COLOUR_LOCK

    # "earthy" in the node backbone was the second warm-bias source, pulling the
    # palette toward soil and sandstone. The icons never suffered from it as
    # badly because their subjects were already neutral.
    spec["node_backbone"] = spec["node_backbone"].replace(
        "muted earthy desaturated colours", "muted desaturated colours")

    changed = 0
    for node in spec["nodes"]:
        fixes = EXTRA_FIXES + (STONE_ONLY_FIXES if node["key"] == "stone" else [])
        for bucket in ("node_prompts", "icon_prompts"):
            for variant in node[bucket]:
                for field, table in (("subject", SUBJECT_FIXES), ("extra", fixes)):
                    before = variant[field]
                    after = fix_extra(before, table)
                    if after != before:
                        changed += 1
                    variant[field] = after

    PROMPTS.write_text(json.dumps(spec, indent=2) + "\n")
    print(f"patched {changed} extra clauses + 2 backbones -> {PROMPTS}", file=sys.stderr)


if __name__ == "__main__":
    main()
