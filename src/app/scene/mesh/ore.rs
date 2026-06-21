//! Ore/vein resource-node deposits are authored Blender glbs, one per ore type
//! and depletion stage (a chunky faceted boulder studded with mineral chunks).
//! Geometry + UVs + per-mineral COLOR_0 come from `art/ore/build_ore.py`; the
//! loader and the four shared StandardMaterials live in
//! `src/app/scene/assets.rs`. This module now only owns the shared stage count.

/// Number of visual depletion stages an ore/vein node steps through while
/// being mined: 0 = untouched, 1 = worn down, 2 = nearly mined out. The
/// fully-empty node despawns with the shatter effect, so there's no
/// "stage 3" mesh.
pub(crate) const ORE_NODE_STAGE_COUNT: usize = 3;
