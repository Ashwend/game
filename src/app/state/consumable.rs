//! Client-side use-charge state for a held consumable (the bandage).
//!
//! Sibling of [`RangedDrawState`](super::ranged::RangedDrawState), and shaped the
//! same way on purpose: a pure input->intent machine, unit-testable in isolation
//! from Bevy. [`ConsumeChargeState::update`] takes the frame's press/release
//! booleans plus the held item's profile and returns a [`ConsumeAction`] the input
//! system turns into a [`crate::protocol::ConsumableCommand`]. It never sends
//! messages and never touches health.
//!
//! ## This clock is for the VIEWMODEL only
//!
//! The fraction tracked here drives the first-person pose and the HUD charge arc.
//! It does **not** decide whether the bandage applies. The server runs the same
//! curve off its own ticks and applies the heal itself (see `server::heal`), so
//! this clock reaching `1.0` is a prediction, not an authority. It deliberately
//! reaches full a frame or two *before* the server does, which is why the pose
//! holds at full rather than snapping: the client waits to be told, by the
//! replicated health going up, that the wrap actually landed.
//!
//! There is correspondingly **no** "apply" message for the client to send. The
//! only two things it can say are "I started" and "I let go".

use crate::items::ConsumableProfile;

/// What the consumable state machine wants the input system to do this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ConsumeAction {
    /// A use just began: send `ConsumableCommand::UseStart`.
    UseStart,
    /// The use was abandoned before completing (button released early, item
    /// swapped, overlay opened, death): send `ConsumableCommand::UseCancel`.
    /// Costs the player nothing; the server only spends the item on completion.
    Cancel,
}

/// A live use charge in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveUse {
    /// Seconds since the use began.
    elapsed: f32,
    /// The full-charge window in seconds (`use_ticks_to_full / tick_rate`).
    use_seconds: f32,
}

/// Client-side consumable use tracker. One per local player, a Bevy resource.
#[derive(bevy::prelude::Resource, Debug, Clone)]
pub(crate) struct ConsumeChargeState {
    /// The active use, if the player is holding the primary attack on a
    /// consumable. `None` otherwise.
    active: Option<ActiveUse>,
    /// Seconds since the last use ended, whether it completed or was abandoned.
    /// Drives the settle so the viewmodel eases back to the carry instead of
    /// snapping. Counts up every frame; large means "not recently used".
    since_end: f32,
    /// The fraction the charge had reached when it ended. The settle blends *from*
    /// this, so an abandoned half-wrap drops from halfway rather than from full.
    ended_at: f32,
    /// DEV-ONLY pose override (headless agent capture). When set, `use_fraction`
    /// and `is_using` report this forced charge instead of the live one, so an
    /// agent can screenshot the mid-wrap viewmodel without the focus-gated mouse
    /// button. Never set outside the dev control socket; `update` clears it if real
    /// input arrives, so it can't wedge live play. Mirrors
    /// [`RangedDrawState::debug_override`](super::ranged::RangedDrawState).
    #[cfg(debug_assertions)]
    debug_use: Option<f32>,
}

/// Seconds for the viewmodel to settle back to the carry pose after a use ends.
const CONSUME_SETTLE_SECONDS: f32 = 0.28;

/// Upper bound on `since_end`, so a long session can't drift the clock. Also the
/// value it *starts* at: see [`ConsumeChargeState::default`].
const SINCE_END_CEILING: f32 = 3600.0;

impl Default for ConsumeChargeState {
    /// A player who has never used a bandage must read as fully SETTLED, not as
    /// having just finished one. A derived `Default` would zero `since_end`, which
    /// means `settle_progress()` starts at 0 (mid-settle) and the viewmodel is
    /// posed as if a use just ended. Start the clock already run out instead.
    fn default() -> Self {
        Self {
            active: None,
            since_end: SINCE_END_CEILING,
            ended_at: 0.0,
            #[cfg(debug_assertions)]
            debug_use: None,
        }
    }
}

impl ConsumeChargeState {
    /// Drive the use state one frame. Returns the [`ConsumeAction`] the input
    /// system should act on, or `None` when nothing changed.
    ///
    /// - `just_pressed` / `pressed`: the primary-attack button edge + held state.
    /// - `profile`: the held item's consumable profile, or `None` when the active
    ///   item is not a consumable.
    ///
    /// Note what is NOT here: any notion of the use *completing*. Holding past
    /// full simply pins the fraction at `1.0` and keeps waiting. The server
    /// decides, and the client learns about it when the item leaves the inventory
    /// and health goes up.
    pub(crate) fn update(
        &mut self,
        delta_seconds: f32,
        just_pressed: bool,
        pressed: bool,
        profile: Option<ConsumableProfile>,
    ) -> Option<ConsumeAction> {
        let delta = delta_seconds.max(0.0);

        // A real press clears any dev pose override so the headless capture hook
        // can never wedge live play.
        #[cfg(debug_assertions)]
        if just_pressed {
            self.debug_use = None;
        }

        // The settle clock always advances (bounded so a long session doesn't
        // drift), so it can never wedge on a swap or holster.
        self.since_end = (self.since_end + delta).min(SINCE_END_CEILING);

        // Not holding a consumable: abandon any use in flight. Covers the
        // item-swap / put-away cases the input layer also guards.
        let Some(profile) = profile else {
            return self.cancel_if_active();
        };

        if let Some(mut active) = self.active {
            // Letting go before the server completes the charge abandons it. The
            // client cannot know for certain whether the server already applied it
            // on this exact tick, and it does not need to: `UseCancel` is a no-op
            // server-side once the use has been cleared by completion, so the
            // worst case is a redundant message, never a double-spend.
            if !pressed {
                return self.end_use();
            }
            active.elapsed += delta;
            self.active = Some(active);
            return None;
        }

        // No use in flight. A fresh press starts one.
        if just_pressed {
            self.active = Some(ActiveUse {
                elapsed: 0.0,
                use_seconds: profile.use_seconds(),
            });
            return Some(ConsumeAction::UseStart);
        }

        None
    }

    /// Abandon any active use (item swap, overlay open, death, holster). Returns
    /// [`ConsumeAction::Cancel`] when a use was actually cleared, so the input
    /// layer sends exactly one `UseCancel`, and `None` when there was nothing to
    /// cancel (idempotent).
    pub(crate) fn cancel_if_active(&mut self) -> Option<ConsumeAction> {
        self.active.is_some().then(|| self.end_use())?
    }

    /// Tear down the active use and start the settle from wherever it got to.
    fn end_use(&mut self) -> Option<ConsumeAction> {
        self.ended_at = self.use_fraction();
        self.active = None;
        self.since_end = 0.0;
        Some(ConsumeAction::Cancel)
    }

    /// Use charge in `[0, 1]`: how far the wrap has progressed. Drives the
    /// viewmodel pose, the tail unroll, and the HUD charge arc. `0` when idle.
    pub(crate) fn use_fraction(&self) -> f32 {
        #[cfg(debug_assertions)]
        if let Some(forced) = self.debug_use {
            return forced.clamp(0.0, 1.0);
        }
        match self.active {
            Some(active) if active.use_seconds > 0.0 => {
                (active.elapsed / active.use_seconds).clamp(0.0, 1.0)
            }
            // A zero-length charge would apply on the press frame; treat it as
            // instantly full rather than dividing by zero.
            Some(_) => 1.0,
            None => 0.0,
        }
    }

    /// True while a use is being held. Gates the HUD charge arc.
    pub(crate) fn is_using(&self) -> bool {
        #[cfg(debug_assertions)]
        if self.debug_use.is_some() {
            return true;
        }
        self.active.is_some()
    }

    /// DEV-ONLY: force the use charge for headless agent capture. `None` clears
    /// back to live input. See [`Self::debug_use`].
    #[cfg(debug_assertions)]
    pub(crate) fn set_debug_use(&mut self, forced: Option<f32>) {
        self.debug_use = forced;
    }

    /// Settle progress in `[0, 1]` since the last use ended (`0` just ended, `1`
    /// fully back at the carry). `1` when idle, so a player who has never used a
    /// bandage sits at rest rather than mid-settle.
    pub(crate) fn settle_progress(&self) -> f32 {
        if self.active.is_some() {
            return 0.0;
        }
        (self.since_end / CONSUME_SETTLE_SECONDS).clamp(0.0, 1.0)
    }

    /// The charge the last use reached before it ended. The settle blends from
    /// here, so an abandoned half-wrap drops back from halfway.
    pub(crate) fn ended_at(&self) -> f32 {
        self.ended_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SERVER_TICK_RATE_HZ;

    fn profile() -> ConsumableProfile {
        ConsumableProfile {
            // 3 s at 20 Hz.
            use_ticks_to_full: (3.0 * SERVER_TICK_RATE_HZ) as u64,
            instant_heal: 15.0,
            heal_over_time: 20.0,
            heal_duration_ticks: 200,
            use_move_multiplier: 0.4,
        }
    }

    #[test]
    fn a_press_starts_a_use_and_the_fraction_ramps_while_held() {
        let mut state = ConsumeChargeState::default();
        assert_eq!(
            state.update(0.0, true, true, Some(profile())),
            Some(ConsumeAction::UseStart)
        );
        assert!(state.is_using());
        assert_eq!(state.use_fraction(), 0.0);

        // Half the 3 s window.
        assert_eq!(state.update(1.5, false, true, Some(profile())), None);
        assert!((state.use_fraction() - 0.5).abs() < 1e-5);
    }

    #[test]
    fn holding_past_full_pins_at_one_and_never_self_completes() {
        let mut state = ConsumeChargeState::default();
        state.update(0.0, true, true, Some(profile()));
        state.update(3.0, false, true, Some(profile()));
        assert_eq!(state.use_fraction(), 1.0);

        // Keep holding well past full: the client STILL just sits at 1.0 and emits
        // nothing. It has no "apply" to send; only the server can complete a use.
        for _ in 0..10 {
            assert_eq!(state.update(0.5, false, true, Some(profile())), None);
        }
        assert_eq!(state.use_fraction(), 1.0);
        assert!(state.is_using());
    }

    #[test]
    fn an_early_release_cancels_and_settles_from_where_it_got_to() {
        let mut state = ConsumeChargeState::default();
        state.update(0.0, true, true, Some(profile()));
        state.update(1.5, false, true, Some(profile()));

        // Release at half charge.
        assert_eq!(
            state.update(0.0, false, false, Some(profile())),
            Some(ConsumeAction::Cancel)
        );
        assert!(!state.is_using());
        assert_eq!(state.use_fraction(), 0.0);
        // The settle blends back from the halfway pose, not from full.
        assert!((state.ended_at() - 0.5).abs() < 1e-5);
        assert_eq!(state.settle_progress(), 0.0);

        // And it eases back to rest over the settle window.
        state.update(CONSUME_SETTLE_SECONDS, false, false, Some(profile()));
        assert_eq!(state.settle_progress(), 1.0);
    }

    #[test]
    fn swapping_away_from_a_consumable_cancels_the_use() {
        let mut state = ConsumeChargeState::default();
        state.update(0.0, true, true, Some(profile()));
        // Held item is no longer a consumable: the use is abandoned even though
        // the button is still down.
        assert_eq!(
            state.update(0.1, false, true, None),
            Some(ConsumeAction::Cancel)
        );
        assert!(!state.is_using());
        // Idempotent: no second cancel.
        assert_eq!(state.update(0.1, false, true, None), None);
    }

    #[test]
    fn cancel_if_active_is_idempotent() {
        let mut state = ConsumeChargeState::default();
        assert_eq!(state.cancel_if_active(), None);
        state.update(0.0, true, true, Some(profile()));
        assert_eq!(state.cancel_if_active(), Some(ConsumeAction::Cancel));
        assert_eq!(state.cancel_if_active(), None);
    }

    #[test]
    fn an_idle_player_sits_settled_at_rest() {
        let state = ConsumeChargeState::default();
        assert!(!state.is_using());
        assert_eq!(state.use_fraction(), 0.0);
        // Fully settled, not mid-settle: someone who has never used a bandage
        // must not be posed as if they just finished one.
        assert_eq!(state.settle_progress(), 1.0);
    }
}
