//! CPU wind sway for the harvestable hay-grass node.
//!
//! The cosmetic detail-grass field bends in its custom instancing shader, but the
//! hay node is a per-entity `StandardMaterial` mesh (it must stay a normal,
//! always-visible, interactable resource node, not dither out with distance like
//! the field), so it can't bend in a shader. Instead each tuft is leaned around
//! its pinned base on the CPU here: a small oscillating tilt phased by world
//! position so neighbouring tufts share a breeze direction, matching the field's
//! wind feel closely enough for a single clump. Hay nodes are far fewer than grass
//! blades, so the per-frame transform write is cheap (it mirrors the pop-in tick).

use bevy::prelude::*;

use super::ResourceNodePopIn;

/// Marker on a harvestable hay-grass node. Carries the tuft's resting rotation so
/// [`sway_hay_grass_system`] can re-derive the lean from a fixed base every frame
/// (rather than accumulating onto the live transform, which would drift).
#[derive(Component)]
pub(crate) struct HayGrass {
    rest_rotation: Quat,
}

impl HayGrass {
    pub(crate) fn new(rest_rotation: Quat) -> Self {
        Self { rest_rotation }
    }
}

/// Lean oscillation speed (rad/s of phase). Kept gentle so the tuft breathes
/// rather than flaps.
const SWAY_SPEED: f32 = 1.1;
/// Peak lean angle (radians, ~5 degrees). The tip of a ~1 m tuft travels a few
/// centimetres, a believable breeze.
const SWAY_AMPLITUDE: f32 = 0.09;

/// Lean every settled hay tuft on a world-position-phased wind wave. Skips tufts
/// still playing their spawn pop-in ([`ResourceNodePopIn`] owns the transform
/// until it finishes), then takes over once the pop-in component is removed.
///
/// Only the rotation is touched (around the entity origin, which sits at the
/// tuft's base on the ground), so the root stays pinned and the tips sway. The
/// lean is recomputed from `rest_rotation` each frame, so it never accumulates.
pub(crate) fn sway_hay_grass_system(
    time: Res<Time>,
    mut hay: Query<(&HayGrass, &mut Transform), Without<ResourceNodePopIn>>,
) {
    let t = time.elapsed_secs();
    for (hay, mut transform) in &mut hay {
        // Phase from world XZ so adjacent tufts lean together like one breeze.
        let p = transform.translation;
        let phase = p.x * 0.18 + p.z * 0.13;
        let lean_x = (t * SWAY_SPEED + phase).sin() * SWAY_AMPLITUDE;
        let lean_z = (t * SWAY_SPEED * 0.83 + phase * 1.4).sin() * SWAY_AMPLITUDE * 0.8;
        transform.rotation =
            hay.rest_rotation * Quat::from_rotation_x(lean_x) * Quat::from_rotation_z(lean_z);
    }
}
