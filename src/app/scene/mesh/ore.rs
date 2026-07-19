//! Ore/vein resource-node deposits are generated glbs, one per ore type and
//! depletion stage (a chunky faceted boulder studded with mineral chunks).
//! Geometry + UVs come from `art/ore/build_nodes.py` (image-to-3D output,
//! retopologised, with the AI albedo rebaked per type); the loader and the
//! five per-type ToonMaterials live in `src/app/scene/assets.rs`. This module
//! now only owns the shared stage count.

/// Number of visual depletion stages an ore/vein node steps through while
/// being mined: 0 = untouched, 1 = worn down, 2 = nearly mined out. The
/// fully-empty node despawns with the shatter effect, so there's no
/// "stage 3" mesh.
pub(crate) const ORE_NODE_STAGE_COUNT: usize = 3;
