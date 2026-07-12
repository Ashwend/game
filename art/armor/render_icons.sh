#!/bin/bash
# Render + finalize all 12 armor icons from their glbs (P4a icon pipeline).
#
# Armor icons share one FRONT-3/4 camera (az 115 / el 17): the hero detail (face
# panels, visor, keel ridge, knee cops) sits on the rig's forward -Z, which the
# weapons' default az 34 camera would show from behind. Fit is the exact
# corner-projection mode (the default radial fit under-frames boxy shapes and
# crops). Exposure is per set:
#   padded   Standard transform, light 0.6, world 0.45  -> warm undyed tan cloth
#   lamellar renderer defaults (AgX, light 1.0)         -> approved wood-slat look
#   iron     Standard, light 0.45, world 0.25, gain 0.5 -> mid-grey steel, dark
#            rivets (COLOR_0 steel is bright on purpose for the in-game F0; the
#            gain is icon-only). AgX would lift+desaturate all three back to the
#            washed-out look, hence Standard for padded/iron.
# Weapon icons are untouched: every env here defaults off in scripts/render_icon.py.
#
# Usage: art/armor/render_icons.sh [id ...]   (no args = all 12)
set -euo pipefail
cd "$(dirname "$0")/../.."

BLENDER=/Applications/Blender.app/Contents/MacOS/Blender
COMMON=(ICON_AZIMUTH_DEG=115 ICON_ELEVATION_DEG=17 ICON_FIT_MODE=corners)
PADDED=(ICON_VIEW_TRANSFORM=Standard ICON_LIGHT_SCALE=0.6 ICON_WORLD_STRENGTH=0.45)
LAMELLAR=()
IRON=(ICON_VIEW_TRANSFORM=Standard ICON_LIGHT_SCALE=0.45 ICON_WORLD_STRENGTH=0.25 ICON_COLOR_GAIN=0.5)

ALL=(padded_hood padded_tunic padded_leggings padded_wraps
     lamellar_helm lamellar_vest lamellar_greaves lamellar_boots
     iron_helm iron_cuirass iron_greaves iron_boots)
IDS=("${@:-${ALL[@]}}")

for id in "${IDS[@]}"; do
  case "$id" in
    padded_*)   SET=("${PADDED[@]}") ;;
    lamellar_*) SET=("${LAMELLAR[@]+"${LAMELLAR[@]}"}") ;;
    iron_*)     SET=("${IRON[@]}") ;;
    *) echo "unknown armor id: $id" >&2; exit 1 ;;
  esac
  env "${COMMON[@]}" ${SET[@]+"${SET[@]}"} "$BLENDER" -b -P scripts/render_icon.py -- \
    "assets/items/$id/model.glb" "art/items/$id/icon_master_512.png" 512 >/dev/null 2>&1
  echo "master  $id"
  python3 scripts/icon_finalize.py --master "art/items/$id/icon_master_512.png" \
    --out "assets/items/$id/icon.png"
done
