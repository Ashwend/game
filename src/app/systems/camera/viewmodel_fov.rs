//! Viewmodel-camera FOV sync for the ranged draw pinch.
//!
//! The first-person viewmodel camera renders the held item in its own pass with a
//! `PerspectiveProjection` that is set once at spawn and never updated (see the
//! spawn in `app::scene::assets`); deliberately so, since the world camera's
//! player-chosen FOV and run boost must not warp the held tool. The bow draw is
//! the exception: when the main camera pinches at full draw, the held bow must
//! visually tighten with it or the hands read as detached from the world zoom.
//!
//! This system keeps a *proportional* pinch on the viewmodel camera while a draw
//! is held: the main camera's smoothed pinch, as a fraction of its base FOV, is
//! applied to the viewmodel's own base FOV. When the pinch decays to zero
//! (release, cancel, item swap, screen change), the projection lands back on
//! exactly [`VIEWMODEL_BASE_FOV_DEG`], so the restore is clean by construction.

use bevy::prelude::*;

use crate::app::scene::ViewmodelCamera;

use super::effects::CameraMotionEffects;

/// The viewmodel camera's own base vertical FOV, in degrees. The spawn in
/// `app::scene::assets` reads this same constant, so the sync below and the
/// spawn value can never drift apart.
pub(crate) const VIEWMODEL_BASE_FOV_DEG: f32 = 65.0;

/// Writes-only-on-change epsilon, in radians. The projection is only rewritten
/// when the target differs by more than this, so an idle (pinch 0) frame does not
/// dirty the projection component every frame.
const FOV_WRITE_EPSILON_RAD: f32 = 1e-4;

/// The viewmodel FOV, in radians, for a main camera currently pinched by
/// `pinch_deg` out of a `base_fov_deg` world FOV. Proportional: the viewmodel
/// tightens by the same *fraction* the world view does, so the two stay in step
/// whatever base FOV the player chose. Pinch zero returns exactly the spawn FOV;
/// a degenerate base (zero / non-finite) is treated as no pinch so the viewmodel
/// can never invert.
pub(crate) fn viewmodel_fov_radians(pinch_deg: f32, base_fov_deg: f32) -> f32 {
    let fraction = if base_fov_deg.is_finite() && base_fov_deg > 0.0 && pinch_deg.is_finite() {
        (pinch_deg / base_fov_deg).clamp(0.0, 0.5)
    } else {
        0.0
    };
    (VIEWMODEL_BASE_FOV_DEG * (1.0 - fraction)).to_radians()
}

/// Keep the viewmodel camera's projection in step with the ranged draw pinch.
/// Runs after `camera_follow_system` (which advances the smoothed pinch) so the
/// two cameras tighten on the same frame.
pub(crate) fn sync_viewmodel_fov_system(
    motion: Res<CameraMotionEffects>,
    mut cameras: Query<&mut Projection, With<ViewmodelCamera>>,
) {
    let target = viewmodel_fov_radians(motion.ranged_pinch_deg(), motion.base_fov_deg());
    for mut projection in &mut cameras {
        if let Projection::Perspective(perspective) = projection.as_mut()
            && (perspective.fov - target).abs() > FOV_WRITE_EPSILON_RAD
        {
            perspective.fov = target;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_pinch_restores_the_exact_spawn_fov() {
        // With no draw held the viewmodel FOV is byte-for-byte the spawn value, so
        // cancel / fire / swap all land the held item back at its authored look.
        assert_eq!(
            viewmodel_fov_radians(0.0, 65.0),
            VIEWMODEL_BASE_FOV_DEG.to_radians()
        );
    }

    #[test]
    fn full_pinch_tightens_the_viewmodel_proportionally() {
        // A 4-degree pinch out of a 65-degree world FOV is a ~6.15% tightening;
        // the viewmodel must tighten by the same fraction of ITS base.
        let pinch = 4.0;
        let base = 65.0;
        let expected = (VIEWMODEL_BASE_FOV_DEG * (1.0 - pinch / base)).to_radians();
        assert!((viewmodel_fov_radians(pinch, base) - expected).abs() < 1e-6);
        assert!(viewmodel_fov_radians(pinch, base) < viewmodel_fov_radians(0.0, base));
    }

    #[test]
    fn proportionality_tracks_the_players_chosen_base_fov() {
        // The same absolute pinch is a LARGER fraction of a narrower world FOV, so
        // the viewmodel tightens more for a player running a low base FOV. This is
        // what keeps the two cameras visually in step at any FOV setting.
        let tight_base = viewmodel_fov_radians(4.0, 50.0);
        let wide_base = viewmodel_fov_radians(4.0, 100.0);
        assert!(tight_base < wide_base);
    }

    #[test]
    fn degenerate_inputs_fall_back_to_the_spawn_fov() {
        // A zero / negative / NaN base (or NaN pinch) must never produce an
        // inverted or NaN projection; it reads as "no pinch".
        for (pinch, base) in [(4.0, 0.0), (4.0, -10.0), (4.0, f32::NAN), (f32::NAN, 65.0)] {
            assert_eq!(
                viewmodel_fov_radians(pinch, base),
                VIEWMODEL_BASE_FOV_DEG.to_radians(),
                "pinch {pinch} base {base} should fall back to the spawn FOV"
            );
        }
    }
}
