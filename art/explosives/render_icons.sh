#!/bin/bash
# Render + finalize the four explosive icons from their glbs (P6a icon
# pipeline). Mesh-rendered, consistent 3/4 view like the weapons/armour sets.
#
# These are upright ground props. The hero detail differs per piece, so the camera
# and exposure are tuned per item:
#   powder_keg     az 34 (default): radially symmetric barrel; the fuse + hoops read
#                  from any azimuth. Corners fit so the tall fuse is not cropped.
#   satchel_charge az 70: the strap + buckle sit on the FRONT (+Y). The default
#                  az-34 shows them edge-on; orbiting to ~70 puts the buckle proud.
#   powder_bomb    az 34 (default): round; the pucker knot + fuse read from anywhere.
#   ember_charge   az 34 (default), no view-transform override, brighter world so the
#                  caged crystal glow reads. It is a colourful icon, so NO desaturate
#                  in finalize (would kill the ember orange, per icon_finalize docs).
#
# Corners fit for all four (the default radial fit under-frames chunky+tall shapes
# and can crop the fuse). LIGHT_SCALE trimmed a touch so the bright iron/steel bits
# do not clip to white under the fuse.
#
# Usage: art/explosives/render_icons.sh [id ...]   (no args = all 4)
set -euo pipefail
cd "$(dirname "$0")/../.."

BLENDER=/Applications/Blender.app/Contents/MacOS/Blender
COMMON=(ICON_FIT_MODE=corners ICON_FIT_MARGIN=1.16)

ALL=(powder_keg satchel_charge powder_bomb ember_charge)
IDS=("${@:-${ALL[@]}}")

for id in "${IDS[@]}"; do
  case "$id" in
    powder_keg)     SET=(ICON_AZIMUTH_DEG=34 ICON_ELEVATION_DEG=18) ;;
    satchel_charge) SET=(ICON_AZIMUTH_DEG=70 ICON_ELEVATION_DEG=17) ;;
    powder_bomb)    SET=(ICON_AZIMUTH_DEG=34 ICON_ELEVATION_DEG=16) ;;
    ember_charge)   SET=(ICON_AZIMUTH_DEG=34 ICON_ELEVATION_DEG=16 ICON_WORLD_STRENGTH=0.7) ;;
    *) echo "unknown explosive id: $id" >&2; exit 1 ;;
  esac
  env "${COMMON[@]}" "${SET[@]}" "$BLENDER" -b -P scripts/render_icon.py -- \
    "assets/items/$id/model.glb" "art/items/$id/icon_master_512.png" 512 >/dev/null 2>&1
  echo "master  $id"
  python3 scripts/icon_finalize.py --master "art/items/$id/icon_master_512.png" \
    --out "assets/items/$id/icon.png"
done
