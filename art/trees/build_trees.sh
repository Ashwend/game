#!/usr/bin/env bash
# Rebuild all nine tree glbs (pine/birch small/medium/large live + 3 dead snags)
# from the parametric builder. Textures must already be in assets/textures/trees/
# (see art/trees/make_tree_textures.py). export_materials='NONE', so the bark/
# foliage args only feed the optional preview render; Rust attaches the shared
# cel ToonMaterials in-game.
#
# Usage:  art/trees/build_trees.sh            (all 9)
#         BLENDER=/path/to/blender art/trees/build_trees.sh
set -euo pipefail
BL="${BLENDER:-/Applications/Blender.app/Contents/MacOS/Blender}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
T="assets/textures/trees"

build() {  # species size foliage_tex
  local sp=$1 sz=$2 fol=$3
  local out="assets/trees/${sp}_${sz}/model.glb"
  mkdir -p "$(dirname "$out")"
  "$BL" --background --python art/trees/build_tree.py -- \
    "$sp" "$sz" "$out" "" "$T/bark_${sp}.png" "$fol" >/dev/null
  printf '  %-14s %6s bytes\n' "$sp/$sz" "$(stat -f%z "$out")"
}

for sz in small medium large; do
  build pine  "$sz" "$T/foliage_pine.png"
  build birch "$sz" "$T/foliage_birch.png"
done
for sz in small medium large; do
  out="assets/trees/dead_${sz}/model.glb"
  mkdir -p "$(dirname "$out")"
  "$BL" --background --python art/trees/build_tree.py -- \
    dead "$sz" "$out" "" "$T/bark_pine.png" "" >/dev/null
  printf '  %-14s %6s bytes\n' "dead/$sz" "$(stat -f%z "$out")"
done
echo "built 9 tree glbs"
