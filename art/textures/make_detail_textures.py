#!/usr/bin/env python3
"""Generate the item detail textures with ComfyUI Flux and heal
them into seamless tiles with OpenCV. Follows art/tools/make_tool_textures.py's
pipeline EXACTLY (generate 1024, seam-heal by roll+feather, neutralize to a light
neutral-luma grain, soften contrast, resize 512) so these tiles ride the mesh
COLOR_0 the same way the tool tiles do: the cel shader supplies the banding and
ink edge and the engine tints via COLOR_0, so the texture carries only grain.

The five base tiles are the padded/lamellar/iron armor line and the explosive
props:
  assets/textures/tools/steel.png        polished forged steel (sword/mace heads, iron armor trim)
  assets/textures/tools/cloth.png        coarse woven cloth weave (padded armor, grips, charge wrapping)
  assets/textures/props/wood_slat.png    flat split-wood slats side by side (lamellar plates)
  assets/textures/props/keg_staves.png   vertical barrel staves with band shadows (powder keg)
  assets/textures/props/meteorite_crystal.png glassy crystal facets, mostly neutral (meteorite + ember charge)

The ember crystal additionally yields a DERIVED emissive mask:
  assets/textures/props/meteorite_crystal_emissive.png  greyscale veins-on-black (see derive_emissive)

The base tiles keep the neutral-luma philosophy (desaturate toward luma, mean ~210)
because the engine tints them. The emissive mask is the one exception: it is a
greyscale control map, not an albedo tile, so it is NOT neutralized; it is a
curved/thresholded vein structure lifted out of the picked crystal tile.

Flux does not tile natively, so we generate at 1024, roll by half and feather the
centre cross (the only seam) toward the smooth interior, then knock the contrast
down so the texture never fights the cel bands.

Workflow:
  # 1. generate candidate seeds for every material (or one) into candidates/<name>/
  python3 art/textures/make_detail_textures.py raw
  python3 art/textures/make_detail_textures.py raw steel

  # 2. visually inspect candidates/<name>/*.png, then heal the winner to its output
  python3 art/textures/make_detail_textures.py final steel steel_b
  python3 art/textures/make_detail_textures.py final cloth cloth_a 0.55

  # 3. derive the emissive mask from the PICKED meteorite_crystal candidate
  python3 art/textures/make_detail_textures.py emissive meteorite_crystal_c
"""
import sys, os
import numpy as np
import cv2

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from comfy_gen import generate

# Two output roots: tool-family tiles live beside the existing tool tiles, prop
# tiles get a new props/ directory.
OUT_TOOLS = "assets/textures/tools"
OUT_PROPS = "assets/textures/props"
CAND = "art/textures/candidates"          # kept for human review, one dir per material
RAW = "/tmp/detail_tex_raw"            # scratch mirror of the raw 1024 gens
os.makedirs(OUT_TOOLS, exist_ok=True)
os.makedirs(OUT_PROPS, exist_ok=True)
os.makedirs(CAND, exist_ok=True)
os.makedirs(RAW, exist_ok=True)

# Same STYLE template make_tool_textures.py uses: flat top-down, even ambient,
# no hotspots, low contrast, hand-painted cel-shaded anime game texture.
STYLE = ("seamless tileable texture, top-down orthographic flat surface, "
         "hand-painted cel-shaded anime game texture, flat even ambient light, "
         "no hotspots, no directional shadow, uniform, low contrast")

# material -> output-dir + list of (variant_suffix, prompt, seed). 3-4 seeds each.
# Prompts describe grain only; the engine owns the base colour via COLOR_0, so we
# still ask for the plausible hue (steel grey, cloth beige, wood tan, ember warm)
# but neutralize it out of the base tiles afterwards.
TARGETS = {
    "steel": {
        "dir": OUT_TOOLS,
        "variants": [
            ("a", "polished forged steel plate surface, subtle brushed grain, "
                  "faint scattered hammer peen marks, cool light steel grey", 121),
            ("b", "smooth forged steel, fine planishing texture, faint long "
                  "scratches, subtle hammer dents, clean cool grey metal", 122),
            ("c", "hand-forged blued steel surface, soft broad value variation, "
                  "very faint hammer marks, smooth toony anime metal", 123),
            ("d", "polished swordsmith steel, gentle satin sheen, faint fine "
                  "grain, minimal marks, clean stylized cool grey", 124),
        ],
    },
    "cloth": {
        "dir": OUT_TOOLS,
        "variants": [
            ("a", "coarse woven cloth fabric, visible warp and weft threads, "
                  "even plain-weave grid, natural undyed beige linen", 131),
            ("b", "rough hand-woven wool cloth, clear crosshatch weave, thick "
                  "fibrous threads, warm oatmeal beige, soft matte", 132),
            ("c", "quilted padded cloth armor fabric, tight even canvas weave, "
                  "visible warp weft, muted tan, hand-painted anime", 133),
            ("d", "burlap linen weave texture, regular basket-weave threads, "
                  "soft neutral beige fabric, stylized game texture", 134),
        ],
    },
    "wood_slat": {
        "dir": OUT_PROPS,
        "variants": [
            ("a", "row of flat split wood slats side by side, tight vertical "
                  "seams between narrow planks, straight grain, warm tan timber", 141),
            ("b", "lamellar wooden armor plates, narrow flat split-wood strips "
                  "aligned in a row, tight gaps, honey oak grain, toony", 142),
            ("c", "vertical split hardwood laths edge to edge, thin seams, "
                  "straight lengthwise grain, mid warm brown wood, flat light", 143),
            ("d", "flat riven wood shingles in a tight row, narrow slats, clean "
                  "vertical seams, pale tan grain, stylized anime texture", 144),
        ],
    },
    "keg_staves": {
        "dir": OUT_PROPS,
        "variants": [
            ("a", "vertical wooden barrel staves side by side, gently curved "
                  "planks, soft shadow gaps between staves, warm brown oak", 151),
            ("b", "powder keg barrel wood staves, tall vertical planks, subtle "
                  "band shadow lines across, aged dark brown timber, toony", 152),
            ("c", "cooper barrel stave wall, vertical curved planks with soft "
                  "seam shadows, weathered mid brown wood, hand-painted anime", 153),
            ("d", "row of curved barrel staves, vertical wood planks, faint "
                  "horizontal iron band shadow, warm oak grain, flat light", 154),
        ],
    },
    "meteorite_crystal": {
        "dir": OUT_PROPS,
        "variants": [
            ("a", "glassy crystalline mineral facets, sharp angular crystal "
                  "planes, faint warm orange veins glowing inside translucent "
                  "grey crystal, mostly neutral, subtle glow", 161),
            ("b", "cluster of translucent crystal facets, faceted glassy planes, "
                  "thin warm ember-orange veins threading through pale grey "
                  "crystal, low saturation, soft internal light", 162),
            ("c", "cracked gemstone crystal surface, angular reflective facets, "
                  "hairline glowing orange veins in cool grey glassy stone, "
                  "mostly desaturated, faint warm cores", 163),
            ("d", "raw crystal cluster facets, sharp geometric planes, delicate "
                  "molten-orange veins through smoky translucent grey crystal, "
                  "neutral base, subtle ember glow", 164),
        ],
    },
}


# --- pipeline helpers, replicated from art/tools/make_tool_textures.py ---------
# (Intentionally copied, not imported at symbol level, so this script is
# self-contained and the tool script stays untouched. Behaviour is identical.)

def make_seamless(img, feather=0.34):
    """Roll by half so the tile edges become smooth interior, then feather the
    resulting centre cross (the former edges) toward the interior."""
    img = img.astype(np.float32)
    h, w = img.shape[:2]
    rolled = np.roll(img, (h // 2, w // 2), axis=(0, 1))
    yy = np.abs(np.arange(h) - h / 2) / (h / 2)
    xx = np.abs(np.arange(w) - w / 2) / (w / 2)
    wy = np.clip(yy / feather, 0, 1)
    wx = np.clip(xx / feather, 0, 1)
    weight = np.minimum(wy[:, None], wx[None, :])[..., None]  # 0 at cross, 1 at edge
    out = rolled * weight + img * (1.0 - weight)
    return np.clip(out, 0, 255).astype(np.uint8)


def soften(img, amount):
    """Pull contrast toward the image's own mean so the tile reads as flat detail
    under the cel bands (amount 0 = unchanged, 1 = flat)."""
    f = img.astype(np.float32)
    mean = f.reshape(-1, 3).mean(0)[None, None, :]
    return np.clip(f * (1 - amount) + mean * amount, 0, 255).astype(np.uint8)


def to_detail(img, desat=0.85, target_mean=210.0):
    """Turn a coloured Flux tile into a near-neutral DETAIL grain map. The cel
    shading multiplies texture * COLOR_0, so the mesh COLOR_0 owns the colour and
    the texture must only carry light value grain. Pull most of the saturation out
    (toward luma) and rescale the mean luminance to a light neutral so it never
    darkens or hue-shifts the COLOR_0. Mirrors the ore rock.png (mean ~0.8)."""
    f = img.astype(np.float32)
    luma = (f * np.array([0.114, 0.587, 0.299])).sum(2, keepdims=True)  # BGR weights
    f = f * (1.0 - desat) + luma * desat
    m = f.mean()
    if m > 1e-3:
        f *= target_mean / m
    return np.clip(f, 0, 255).astype(np.uint8)


# --- emissive mask derivation (new, meteorite_crystal only) -----------------------

def derive_emissive(picked_variant):
    """Derive a greyscale emissive mask (bright veins on black) from the PICKED
    meteorite_crystal candidate so an engine emissive tint can light only the veins.

    Recipe (documented so a human can re-tune):
      1. Read the picked 1024 candidate and roll it half+half (make_seamless with
         no feather) so the mask tiles with the same phase as the healed albedo.
      2. Isolate the WARM vein signal: in BGR the veins are orange (high R, low B).
         Take a warmth channel = R - B, which is strongly positive only on the
         glowing veins and ~0 on the neutral grey crystal body.
      3. Normalize that warmth to 0..255, then apply a hard-ish gamma/curve
         (pow 1.8) to crush the mid grey body toward black and keep only the
         brightest vein cores, then a low threshold floor to kill residual haze.
      4. Light blur to soften vein edges (emissive bloom likes soft cores),
         resize to 512, write single-channel greyscale. Black = no emission,
         white = full vein glow; the engine multiplies this by a warm ember tint.
    """
    raw = f"{RAW}/{picked_variant}.png" if not picked_variant.endswith(".png") else picked_variant
    img = cv2.imread(raw).astype(np.float32)
    # phase-match the tiling roll (no feather: this is a control map, hard tiling
    # is fine and feathering would smear vein cores)
    h, w = img.shape[:2]
    img = np.roll(img, (h // 2, w // 2), axis=(0, 1))
    b, g, r = img[..., 0], img[..., 1], img[..., 2]
    warmth = r - b                      # positive on orange veins, ~0 on grey
    warmth = np.clip(warmth, 0, None)
    mx = warmth.max()
    if mx > 1e-3:
        warmth = warmth / mx            # 0..1
    warmth = np.power(warmth, 1.8)      # crush mids, keep bright vein cores
    warmth = np.clip((warmth - 0.12) / 0.88, 0, 1)  # threshold floor kills haze
    mask = (warmth * 255).astype(np.uint8)
    mask = cv2.GaussianBlur(mask, (0, 0), 1.2)      # soft cores for bloom
    mask = cv2.resize(mask, (512, 512), interpolation=cv2.INTER_AREA)
    dst = f"{OUT_PROPS}/meteorite_crystal_emissive.png"
    cv2.imwrite(dst, mask)
    print(f"emissive meteorite_crystal_emissive: {dst}  (from {raw}, "
          f"warmth=R-B, gamma 1.8, floor 0.12, blur 1.2)")


# --- drivers ------------------------------------------------------------------

def gen_raw(materials):
    for m in materials:
        cand_dir = f"{CAND}/{m}"
        os.makedirs(cand_dir, exist_ok=True)
        for suffix, prompt, seed in TARGETS[m]["variants"]:
            name = f"{m}_{suffix}"
            raw = f"{RAW}/{name}.png"
            generate(f"{prompt}, {STYLE}", raw, 1024, 1024, 4, seed)
            # keep a copy in candidates/ for later human review
            cv2.imwrite(f"{cand_dir}/{name}.png", cv2.imread(raw))
            print(f"raw {name}: {raw}  (candidate {cand_dir}/{name}.png)", flush=True)


def finalize(material, variant, contrast=0.50, desat=0.85, target_mean=210.0):
    raw = f"{RAW}/{variant}.png" if not variant.endswith(".png") else variant
    img = cv2.imread(raw)
    img = make_seamless(img)
    img = to_detail(img, desat=desat, target_mean=target_mean)
    img = soften(img, contrast)
    img = cv2.resize(img, (512, 512), interpolation=cv2.INTER_AREA)
    out_dir = TARGETS[material]["dir"]
    dst = f"{out_dir}/{material}.png"
    cv2.imwrite(dst, img)
    print(f"final {material}: {dst}  (from {raw}, contrast {contrast}, "
          f"desat {desat}, mean {target_mean})")


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "raw"
    if mode == "raw":
        mats = sys.argv[2:] or list(TARGETS)
        gen_raw(mats)
    elif mode == "final":
        material = sys.argv[2]
        variant = sys.argv[3]
        contrast = float(sys.argv[4]) if len(sys.argv) > 4 else 0.50
        finalize(material, variant, contrast)
    elif mode == "emissive":
        derive_emissive(sys.argv[2])
    else:
        print(f"unknown mode {mode!r}; use 'raw', 'final', or 'emissive'")
