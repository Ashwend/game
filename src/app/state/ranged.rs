//! Client-side draw/reload state for a held ranged weapon (bow, crossbow).
//!
//! Parallel to [`GatherInputState`](super::gather::GatherInputState), which owns
//! the one-shot melee swing. A ranged weapon does not swing: it *holds* a draw
//! (bow) or fires immediately and then *reloads* over a cooldown (crossbow), so
//! it needs its own small state machine rather than the fixed-duration swing.
//!
//! This resource is deliberately a pure input->intent machine, unit-testable in
//! isolation from Bevy: [`RangedDrawState::update`] takes the frame's
//! press/release booleans plus the held weapon's profile and returns a
//! [`RangedAction`] the input system turns into a [`crate::protocol::RangedCommand`].
//! It never sends messages or reads the world itself; the server owns the shot.
//!
//! The draw *fraction* it tracks (ticks-since-start / draw-ticks-to-full, clamped
//! to `[0, 1]`) drives the first-person draw pose and the HUD charge arc. For a
//! crossbow the draw window is zero, so the fraction is instead a *reload*
//! fraction: how far through the cooldown the reload is, which drives the
//! reload pose.

use crate::{items::RangedProfile, protocol::SERVER_TICK_RATE_HZ};

/// What the ranged state machine wants the input system to do this frame. The
/// input system maps each to a [`crate::protocol::RangedCommand`] (or, for
/// [`RangedAction::DryClick`], a local audio cue with no wire traffic).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RangedAction {
    /// A draw just began: send `RangedCommand::DrawStart`.
    /// For a crossbow (no draw hold) this is emitted the same frame as
    /// [`RangedAction::Fire`] so the server sees the same DrawStart+Fire ordering
    /// a bow produces.
    DrawStart,
    /// The shot released this frame: send `RangedCommand::Fire { aim_dir }` and
    /// play the release cue. Carries the draw fraction at release so the
    /// own-arrow prediction launches at the same draw-scaled speed the server
    /// computes (`1.0` for an instant-fire crossbow).
    Fire { draw_fraction: f32 },
    /// A draw was abandoned without firing (item swap, overlay open, weapon put
    /// away, or a release below the minimum firing draw): send
    /// `RangedCommand::DrawCancel`.
    Cancel,
    /// The trigger was pulled while the weapon can't fire (crossbow still on
    /// reload cooldown, or no arrow): play the quiet dry-click, send nothing.
    DryClick,
}

/// A live draw in progress (bow) or the pre-fire instant (crossbow). Holds the
/// archetype flags and the elapsed draw time so the fraction can be derived.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveDraw {
    /// Seconds since the draw began. Converted to ticks (times `SERVER_TICK_RATE_HZ`)
    /// against the profile's `draw_ticks_to_full` to get the fraction.
    elapsed: f32,
    /// The full-draw window in seconds (`draw_ticks_to_full / tick_rate`). Zero
    /// for a crossbow (no draw hold), which fires on press.
    draw_seconds: f32,
    /// True for a crossbow: fires immediately on press rather than holding a draw.
    instant_fire: bool,
}

/// Client-side ranged draw/reload tracker. One per local player, a Bevy resource.
#[derive(bevy::prelude::Resource, Debug, Default, Clone)]
pub(crate) struct RangedDrawState {
    /// The active draw, if the player is currently holding the primary attack on a
    /// ranged weapon. `None` between shots and while not aiming.
    active: Option<ActiveDraw>,
    /// Seconds of reload cooldown left after firing (mirrors the server's
    /// `cooldown_ticks` locally so the client can dry-click and drive the reload
    /// pose without waiting for a server round trip). Counts down every frame.
    reload_remaining: f32,
    /// The full reload window in seconds for the last-fired weapon, so the reload
    /// *fraction* (`1 - remaining/window`) can drive the crossbow crank pose.
    /// Zero when no reload is in flight.
    reload_window: f32,
    /// Whether the last-fired weapon was a crossbow, so only the crossbow drives
    /// the reload pose (a bow's tiny post-fire floor is not a reload).
    reload_is_crossbow: bool,
    /// Seconds since the last shot was fired (both bow and crossbow). Drives the
    /// bow's release flick and the crossbow's recoil kick, which decay over their
    /// own short windows independent of the reload cooldown. Counts up every frame;
    /// large means "not recently fired".
    since_fire: f32,
    /// Crossbow aim-down-sights fraction in `[0, 1]`: eases toward `1` while the
    /// aim (right mouse) button is held with a ready crossbow, back to `0` when
    /// released / reloading / holstered. Drives the ADS viewmodel centring and
    /// the FOV pinch. Client feel only; the server never sees the aim state.
    aim: f32,
    /// DEV-ONLY pose override (headless agent capture). When set, the pose
    /// accessors (`draw_fraction`, `is_drawing`, `reload_fraction`, `recoil`) return
    /// these forced values instead of the live draw/reload state, so an agent can
    /// screenshot the animated bow / crossbow viewmodel without the focus-gated
    /// mouse button. Never set outside the dev control socket; `update` clears it if
    /// real input ever arrives so it can't wedge live play.
    #[cfg(debug_assertions)]
    debug_override: Option<RangedPoseOverride>,
}

/// A forced ranged pose for headless agent capture (dev-only). See
/// [`RangedDrawState::debug_override`].
#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RangedPoseOverride {
    /// Forced bow draw fraction (`Some` => hold a draw at this fraction).
    pub(crate) draw: Option<f32>,
    /// Forced crossbow reload fraction.
    pub(crate) reload: Option<f32>,
    /// Forced crossbow recoil.
    pub(crate) recoil: Option<f32>,
    /// Forced crossbow aim-down-sights fraction.
    pub(crate) aim: Option<f32>,
}

/// Seconds for the crossbow ADS ease from carry to fully aimed (and back).
/// Short enough to feel responsive, long enough that the viewmodel visibly
/// slides to the eye line rather than teleporting.
const CROSSBOW_ADS_EASE_SECONDS: f32 = 0.14;

impl RangedDrawState {
    /// Drive the ranged state one frame. Returns the [`RangedAction`] the input
    /// system should act on this frame (send a command or play a local cue), or
    /// `None` when nothing changed.
    ///
    /// - `just_pressed` / `pressed`: the primary-attack button edge + held state.
    /// - `profile`: the held ranged weapon's profile, or `None` when the active
    ///   item is not a ranged weapon (bare hands, a tool, a melee weapon).
    /// - `has_ammo`: whether the player has at least one arrow (client-side count,
    ///   the server re-checks). A draw can't start, and a bow can't fire, without it.
    ///
    /// The reload cooldown always advances (even when the weapon is holstered), so
    /// it can never get stuck; the draw only lives while a ranged weapon is held.
    pub(crate) fn update(
        &mut self,
        delta_seconds: f32,
        just_pressed: bool,
        pressed: bool,
        profile: Option<RangedProfile>,
        has_ammo: bool,
    ) -> Option<RangedAction> {
        let delta = delta_seconds.max(0.0);

        // A real press clears any dev pose override so the headless capture hook
        // can never wedge live play.
        #[cfg(debug_assertions)]
        if just_pressed {
            self.debug_override = None;
        }

        // Time-since-fire always advances (bounded so a long session doesn't drift),
        // driving the release flick + recoil decay.
        self.since_fire = (self.since_fire + delta).min(3600.0);

        // Reload cooldown always burns down so it can't wedge on a swap/holster.
        if self.reload_remaining > 0.0 {
            self.reload_remaining = (self.reload_remaining - delta).max(0.0);
            if self.reload_remaining <= 0.0 {
                self.reload_window = 0.0;
                self.reload_is_crossbow = false;
            }
        }

        // No ranged weapon in hand: any active draw is abandoned (this covers the
        // item-swap / put-away cases the input layer also guards).
        let Some(profile) = profile else {
            return self.cancel_if_active();
        };

        if let Some(mut active) = self.active {
            // A draw is in flight. Releasing the button fires, unless the draw
            // is still under the minimum firing threshold, in which case the
            // release is a cancel (the bow lowers, no tap-shot; the server
            // enforces the same gate off its own ticks). Keeping the button
            // held advances the draw (fraction / pose / audio milestones).
            if !pressed {
                self.active = None;
                let draw_fraction = if active.instant_fire || active.draw_seconds <= 0.0 {
                    1.0
                } else {
                    (active.elapsed / active.draw_seconds).clamp(0.0, 1.0)
                };
                if !active.instant_fire
                    && draw_fraction < crate::game_balance::BOW_MIN_DRAW_FRACTION_TO_FIRE
                {
                    return Some(RangedAction::Cancel);
                }
                return Some(RangedAction::Fire { draw_fraction });
            }
            active.elapsed += delta;
            self.active = Some(active);
            // A crossbow's "draw" is instantaneous: it should already have fired on
            // the press below, so an instant-fire draw lingering here is a no-op.
            return None;
        }

        // No draw in flight. A fresh press may start one.
        if just_pressed {
            let on_cooldown = self.reload_remaining > 0.0;
            let instant_fire = profile.draw_ticks_to_full == 0;
            if !has_ammo || on_cooldown {
                // Nothing to loose: dry-click. (No ammo, or the crossbow is still
                // cranking.) A bow with a spare arrow is never on cooldown long
                // enough to matter, but the guard keeps the rule uniform.
                return Some(RangedAction::DryClick);
            }
            let draw_seconds = profile.draw_ticks_to_full as f32 / SERVER_TICK_RATE_HZ;
            self.active = Some(ActiveDraw {
                elapsed: 0.0,
                draw_seconds,
                instant_fire,
            });
            // A crossbow fires the instant it is pressed (there is no draw to
            // hold): the input layer emits DrawStart then Fire back to back. The
            // bow just starts its draw and waits for release.
            return Some(RangedAction::DrawStart);
        }

        None
    }

    /// Abandon any active draw (item swap, overlay open, death, holster). Returns
    /// [`RangedAction::Cancel`] when a draw was actually cleared so the input
    /// layer sends exactly one `DrawCancel`, and `None` when there was nothing to
    /// cancel (idempotent).
    pub(crate) fn cancel_if_active(&mut self) -> Option<RangedAction> {
        if self.active.take().is_some() {
            Some(RangedAction::Cancel)
        } else {
            None
        }
    }

    /// Whether a crossbow (instant-fire) draw is active this frame, so the input
    /// layer knows to emit the immediate `Fire` right after the `DrawStart`.
    pub(crate) fn active_is_instant_fire(&self) -> bool {
        self.active.map(|a| a.instant_fire).unwrap_or(false)
    }

    /// Arm the post-fire reload cooldown from the fired weapon's profile. Called by
    /// the input layer the instant it sends a `Fire`, so the client can dry-click
    /// and drive the reload pose without a server round trip. The server is still
    /// authoritative; this only paces local feel.
    pub(crate) fn begin_reload(&mut self, profile: RangedProfile) {
        // Firing ends any draw. A bow release already cleared it in `update`, but
        // the crossbow's instant fire leaves the press-frame draw marker set; a
        // fired shot must never leave a live draw behind (it would emit a second
        // `Fire` when the button is eventually released).
        self.active = None;
        let seconds = profile.cooldown_ticks as f32 / SERVER_TICK_RATE_HZ;
        self.reload_remaining = seconds;
        self.reload_window = seconds;
        self.reload_is_crossbow = profile.draw_ticks_to_full == 0;
        // Restart the release-flick / recoil clock: the shot just fired.
        self.since_fire = 0.0;
    }

    /// Draw fraction in `[0, 1]`: how far a bow draw has progressed toward full.
    /// Drives the draw pose, the HUD charge arc, and the tremble ramp. Always `0`
    /// for a crossbow (no draw window) and `0` when no draw is active.
    pub(crate) fn draw_fraction(&self) -> f32 {
        #[cfg(debug_assertions)]
        if let Some(o) = self.debug_override.and_then(|o| o.draw) {
            return o.clamp(0.0, 1.0);
        }
        match self.active {
            Some(active) if active.draw_seconds > 0.0 => {
                (active.elapsed / active.draw_seconds).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }

    /// True while a draw is being held (a bow at any fraction). Used to gate the
    /// FOV pinch and the draw pose. A crossbow never "holds", so this is false for
    /// it even on the fire frame.
    pub(crate) fn is_drawing(&self) -> bool {
        #[cfg(debug_assertions)]
        if let Some(o) = self.debug_override {
            return o.draw.is_some();
        }
        self.active.map(|a| !a.instant_fire).unwrap_or(false)
    }

    /// Release-flick progress in `[0, 1]` over `window_seconds` since the last
    /// shot (`0` just fired, `1` settled). Drives the bow's release pose. The pose
    /// window constant lives beside the pose system, so it is passed in.
    pub(crate) fn release_progress(&self, window_seconds: f32) -> f32 {
        if window_seconds <= 0.0 {
            return 1.0;
        }
        (self.since_fire / window_seconds).clamp(0.0, 1.0)
    }

    /// Recoil in `[0, 1]` over `window_seconds` since the last shot (`1` just
    /// fired, `0` settled). Drives the crossbow's fire kick. The window constant
    /// lives beside the pose system, so it is passed in.
    pub(crate) fn recoil(&self, window_seconds: f32) -> f32 {
        #[cfg(debug_assertions)]
        if let Some(o) = self.debug_override.and_then(|o| o.recoil) {
            return o.clamp(0.0, 1.0);
        }
        if window_seconds <= 0.0 {
            return 0.0;
        }
        (1.0 - self.since_fire / window_seconds).clamp(0.0, 1.0)
    }

    /// Reload fraction in `[0, 1]`: how far through the crossbow reload the crank
    /// is (`0` just fired, `1` ready). Drives the reload pose and the HUD reload
    /// bar. Zero for a bow (its post-fire floor is not a reload) and when idle.
    pub(crate) fn reload_fraction(&self) -> f32 {
        #[cfg(debug_assertions)]
        if let Some(o) = self.debug_override.and_then(|o| o.reload) {
            return o.clamp(0.0, 1.0);
        }
        if !self.reload_is_crossbow || self.reload_window <= 0.0 {
            return 0.0;
        }
        (1.0 - self.reload_remaining / self.reload_window).clamp(0.0, 1.0)
    }

    /// DEV-ONLY: force the ranged pose for headless agent capture. Set `None` to
    /// clear the override back to live input. See [`RangedPoseOverride`].
    #[cfg(debug_assertions)]
    pub(crate) fn set_debug_override(&mut self, over: Option<RangedPoseOverride>) {
        self.debug_override = over;
    }

    /// Ease the crossbow aim-down-sights fraction one frame: toward `1` while
    /// `aiming`, back toward `0` otherwise. The input layer computes `aiming`
    /// (right mouse held with a READY crossbow); every no-aim path (overlay
    /// open, holstered, reloading, dead) funnels through `aiming == false`, so
    /// the ADS always eases back out through this one decay.
    pub(crate) fn tick_aim(&mut self, delta_seconds: f32, aiming: bool) {
        let step = (delta_seconds.max(0.0) / CROSSBOW_ADS_EASE_SECONDS).min(1.0);
        if aiming {
            self.aim = (self.aim + step).min(1.0);
        } else {
            self.aim = (self.aim - step).max(0.0);
        }
    }

    /// Crossbow aim-down-sights fraction in `[0, 1]` (`0` carry, `1` fully
    /// aimed). Drives the ADS viewmodel centring and the FOV pinch.
    pub(crate) fn aim_fraction(&self) -> f32 {
        #[cfg(debug_assertions)]
        if let Some(o) = self.debug_override.and_then(|o| o.aim) {
            return o.clamp(0.0, 1.0);
        }
        self.aim
    }

    /// True while the crossbow is mid-reload (crank cycle in progress). Drives the
    /// reload pose and the HUD reload bar visibility. A dev pose override that
    /// forces a reload fraction also reports as reloading, so a headless capture of
    /// the reload pose shows the HUD reload bar too (the override is the only way
    /// to screenshot the reload state, since the real reload is driven by the
    /// focus-gated fire button).
    pub(crate) fn is_reloading(&self) -> bool {
        #[cfg(debug_assertions)]
        if let Some(o) = self.debug_override {
            return o.reload.is_some();
        }
        self.reload_is_crossbow && self.reload_remaining > 0.0
    }
}

#[cfg(test)]
mod tests;
