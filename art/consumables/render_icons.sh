#!/bin/bash
# Render + finalize the consumable icons from their glbs. Mesh-rendered, on the
# same consistent 3/4 view as the weapons / armour / explosives sets.
#
#   bandage  az 34 (default): the hero detail is the spiral COIL on the end face,
#            which faces +/-X. The default azimuth catches the coil AND lets the
#            tail run away to the right, so the roll reads as a roll and not a
#            barrel. Corners fit because the tail sticks well out of the roll's
#            radial bound and the default radial fit crops it.
#
# It is a warm, low-saturation icon, so NO --desaturate in finalize (it would
# drain the linen to a dead grey, per scripts/icon_finalize.py's own warning).
#
# Usage: art/consumables/render_icons.sh [id ...]   (no args = all)
set -euo pipefail
cd "$(dirname "$0")/../.."

BLENDER=/Applications/Blender.app/Contents/MacOS/Blender
COMMON=(ICON_FIT_MODE=corners ICON_FIT_MARGIN=1.16)

ALL=(bandage)
IDS=("${@:-${ALL[@]}}")

for id in "${IDS[@]}"; do
  case "$id" in
    bandage) SET=(ICON_AZIMUTH_DEG=34 ICON_ELEVATION_DEG=20) ;;
    *) echo "unknown consumable id: $id" >&2; exit 1 ;;
  esac
  env "${COMMON[@]}" "${SET[@]}" "$BLENDER" -b -P scripts/render_icon.py -- \
    "assets/items/$id/model.glb" "art/items/$id/icon_master_512.png" 512 >/dev/null 2>&1
  echo "master  $id"
  python3 scripts/icon_finalize.py --master "art/items/$id/icon_master_512.png" \
    --out "assets/items/$id/icon.png"
done
