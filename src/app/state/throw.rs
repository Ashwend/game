//! Client-side charge state for the thrown powder bomb: hold left click to
//! wind up, release to throw with power scaled by the held charge.
//!
//! Parallel to [`RangedDrawState`](super::ranged::RangedDrawState) (the bow's
//! draw hold) and deliberately the same pure input->intent shape: `update`
//! takes the frame's press/release booleans and returns a [`ThrowAction`] the
//! input layer acts on. It never sends messages itself; the server owns the
//! throw (velocity clamp, ballistics, fuse, blast).
//!
//! The wind-up drives three things: the first-person wind-up pose (the held
//! bomb pulls back to the shoulder as the charge builds), the HUD charge bar
//! (the bow-draw bar reused), and the `power` field on
//! [`crate::protocol::ExplosiveCommand::Throw`]. A release under
//! [`POWDER_BOMB_MIN_THROW_FRACTION`] cancels instead of throwing, so a stray
//! tap never lobs a bomb at your own feet.

use crate::game_balance::{POWDER_BOMB_CHARGE_SECONDS, POWDER_BOMB_MIN_THROW_FRACTION};

/// What the throw state machine wants the input layer to do this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ThrowAction {
    /// The charge released at or above the minimum: prime the toss animation
    /// and send the throw at its release frame with this power in `[min, 1]`.
    Release { power: f32 },
    /// The charge was abandoned (released under the minimum, item swapped,
    /// overlay opened, death): no throw; the pose eases back to idle.
    Cancel,
}

/// How fast the wind-up pose eases back to idle after a cancel, in seconds
/// from a full wind-up. Purely cosmetic; the charge itself is already gone.
const SETTLE_SECONDS: f32 = 0.12;

/// Client-side bomb throw-charge tracker. One per local player, a Bevy resource.
#[derive(bevy::prelude::Resource, Debug, Default, Clone)]
pub(crate) struct ThrowChargeState {
    /// Seconds the charge has been held, `Some` while left click is down on a
    /// thrown explosive. Fraction = `elapsed / POWDER_BOMB_CHARGE_SECONDS`,
    /// clamped to `[0, 1]`.
    active: Option<f32>,
    /// Wind-up level the pose eases back down from after a cancel (starts at
    /// the fraction at the moment of cancel, decays to zero over
    /// [`SETTLE_SECONDS`]). Zero when idle or charging.
    settle: f32,
    /// Power stashed at release until the toss animation reaches its release
    /// frame, where the input layer takes it and sends the throw.
    pending_power: Option<f32>,
}

impl ThrowChargeState {
    /// Drive the charge one frame. `just_pressed` / `pressed` are the primary
    /// attack button edge + held state; the caller only invokes this while a
    /// thrown explosive is the active item (and calls [`Self::cancel`] the
    /// moment it is not).
    pub(crate) fn update(
        &mut self,
        delta_seconds: f32,
        just_pressed: bool,
        pressed: bool,
    ) -> Option<ThrowAction> {
        let delta = delta_seconds.max(0.0);
        if self.settle > 0.0 {
            self.settle = (self.settle - delta / SETTLE_SECONDS).max(0.0);
        }

        if let Some(elapsed) = self.active {
            if !pressed {
                self.active = None;
                let fraction = charge_fraction_for(elapsed);
                if fraction < POWDER_BOMB_MIN_THROW_FRACTION {
                    // Under the minimum: the bomb lowers, nothing is thrown.
                    self.settle = fraction;
                    return Some(ThrowAction::Cancel);
                }
                return Some(ThrowAction::Release { power: fraction });
            }
            self.active = Some(elapsed + delta);
            return None;
        }

        if just_pressed {
            self.active = Some(0.0);
        }
        None
    }

    /// Abandon any live charge without throwing (item swap, overlay, death).
    /// Also drops a stashed pending power so a queued toss can't fire later.
    pub(crate) fn cancel(&mut self) {
        if let Some(elapsed) = self.active.take() {
            self.settle = charge_fraction_for(elapsed);
        }
        self.pending_power = None;
    }

    /// True while left click is held building up a throw.
    pub(crate) fn is_charging(&self) -> bool {
        self.active.is_some()
    }

    /// The live charge fraction in `[0, 1]` (zero when not charging). Drives
    /// the HUD charge bar.
    pub(crate) fn charge_fraction(&self) -> f32 {
        self.active.map(charge_fraction_for).unwrap_or(0.0)
    }

    /// The wind-up level the first-person pose should show this frame: the
    /// live charge fraction while charging, the decaying settle level right
    /// after a cancel, zero otherwise.
    pub(crate) fn wind_up(&self) -> f32 {
        if let Some(elapsed) = self.active {
            charge_fraction_for(elapsed)
        } else {
            self.settle
        }
    }

    /// Stash the released power until the toss animation's release frame.
    pub(crate) fn stash_power(&mut self, power: f32) {
        self.pending_power = Some(power.clamp(0.0, 1.0));
    }

    /// Take the stashed power (once), at the toss's release frame.
    pub(crate) fn take_pending_power(&mut self) -> Option<f32> {
        self.pending_power.take()
    }
}

fn charge_fraction_for(elapsed: f32) -> f32 {
    if POWDER_BOMB_CHARGE_SECONDS <= 0.0 {
        return 1.0;
    }
    (elapsed / POWDER_BOMB_CHARGE_SECONDS).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_under_the_minimum_cancels_instead_of_throwing() {
        let mut state = ThrowChargeState::default();
        assert_eq!(state.update(0.016, true, true), None);
        assert!(state.is_charging());
        // Hold one more frame (so a little wind-up exists), then release:
        // fraction still well under the minimum.
        assert_eq!(state.update(0.016, false, true), None);
        let action = state.update(0.016, false, false);
        assert_eq!(action, Some(ThrowAction::Cancel));
        assert!(!state.is_charging());
        // The pose eases back down from the abandoned wind-up.
        assert!(state.wind_up() > 0.0);
    }

    #[test]
    fn a_held_charge_releases_with_its_fraction_as_power() {
        let mut state = ThrowChargeState::default();
        state.update(0.0, true, true);
        // Hold for half the charge window.
        let half = POWDER_BOMB_CHARGE_SECONDS * 0.5;
        state.update(half, false, true);
        let action = state.update(0.0, false, false);
        match action {
            Some(ThrowAction::Release { power }) => {
                assert!(
                    (power - 0.5).abs() < 0.05,
                    "half hold => ~half power, got {power}"
                );
            }
            other => panic!("expected a release, got {other:?}"),
        }
    }

    #[test]
    fn charge_clamps_at_full_power() {
        let mut state = ThrowChargeState::default();
        state.update(0.0, true, true);
        state.update(POWDER_BOMB_CHARGE_SECONDS * 5.0, false, true);
        assert_eq!(state.charge_fraction(), 1.0);
        let action = state.update(0.0, false, false);
        assert_eq!(action, Some(ThrowAction::Release { power: 1.0 }));
    }

    #[test]
    fn cancel_drops_a_stashed_pending_power() {
        let mut state = ThrowChargeState::default();
        state.stash_power(0.8);
        state.cancel();
        assert_eq!(state.take_pending_power(), None);
    }
}
