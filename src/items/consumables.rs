//! Consumable-item stats: the charge-to-use healing items (bandage today).
//!
//! A consumable is the first item archetype that is neither swung, fired, nor
//! placed. It is *held down* to charge, and the effect lands only when the
//! charge completes; letting go early abandons it and costs nothing. That
//! shape deliberately mirrors the bow's draw (see [`crate::items::RangedProfile`]),
//! and for the same reason: **the server owns the charge clock**. It records the
//! tick the use began and re-derives the fraction from its own ticks, so a
//! forged "I finished charging" can never grant a free heal.
//!
//! Healing itself is split in two, and both halves ride the one
//! `apply_player_heal` tail on the server:
//!
//! - an **instant** chunk the moment the wrap goes on, so using a bandage has a
//!   felt payoff rather than a silent wait, and
//! - a **heal-over-time** remainder that trickles in afterwards, so a bandage is
//!   worth using *before* a fight rather than only as a panic button mid-swing.
//!
//! The over-time half is what makes the item a real decision: it rewards
//! disengaging, and it is worthless if you die two seconds later.

use serde::{Deserialize, Serialize};

use crate::protocol::SERVER_TICK_RATE_HZ;

/// Stats for an item that is held down to use and heals on completion.
///
/// Balance values live in [`crate::game_balance`], never inline here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConsumableProfile {
    /// Ticks the use button must be held for the effect to land. The server
    /// records the start tick and only applies the effect once this many ticks
    /// have actually elapsed on *its* clock, so the charge cannot be forged.
    /// Must be > 0: a zero-charge consumable would apply on the press frame,
    /// which no item wants and which `use_fraction` would divide by.
    pub use_ticks_to_full: u64,
    /// Health restored the instant the use completes.
    pub instant_heal: f32,
    /// Total additional health restored gradually over `heal_duration_ticks`
    /// after the use completes. Refreshed, not stacked, if a second one is used
    /// while the first is still trickling.
    pub heal_over_time: f32,
    /// How long the [`Self::heal_over_time`] remainder takes to fully land.
    pub heal_duration_ticks: u64,
    /// Movement speed multiplier while charging the use. Below 1.0 so you cannot
    /// sprint away mid-wrap: committing to a bandage has to cost you tempo, or
    /// the item is a free reset button in a chase.
    pub use_move_multiplier: f32,
}

impl ConsumableProfile {
    /// How far along the charge is, in `[0, 1]`, after `use_ticks` held.
    ///
    /// The server drives this from its own observed ticks; the client runs the
    /// same curve off a local seconds clock purely to pose the viewmodel.
    pub fn use_fraction(&self, use_ticks: u64) -> f32 {
        if self.use_ticks_to_full == 0 {
            return 1.0;
        }
        (use_ticks as f32 / self.use_ticks_to_full as f32).clamp(0.0, 1.0)
    }

    /// Whether a use held for `use_ticks` has completed and should apply.
    ///
    /// Unlike the bow, which fires at any draw past a *minimum* and scales the
    /// damage by how far it got, a consumable is all-or-nothing: it applies at
    /// full charge or not at all. That is what makes an early release a clean
    /// cancel with no cost.
    pub fn use_completes(&self, use_ticks: u64) -> bool {
        use_ticks >= self.use_ticks_to_full
    }

    /// The per-tick health trickle of the over-time half. Zero when the item has
    /// no over-time component or a zero-length window (guards the division).
    pub fn heal_per_tick(&self) -> f32 {
        if self.heal_duration_ticks == 0 {
            return 0.0;
        }
        self.heal_over_time / self.heal_duration_ticks as f32
    }

    /// Total health this consumable restores if the full over-time window is
    /// allowed to run out. Used by the item tooltip.
    pub fn total_heal(&self) -> f32 {
        self.instant_heal + self.heal_over_time
    }

    /// The charge window in seconds, for client-side pacing and tooltip copy.
    pub fn use_seconds(&self) -> f32 {
        self.use_ticks_to_full as f32 / SERVER_TICK_RATE_HZ
    }

    /// The over-time window in seconds, for tooltip copy.
    pub fn heal_seconds(&self) -> f32 {
        self.heal_duration_ticks as f32 / SERVER_TICK_RATE_HZ
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bandage() -> ConsumableProfile {
        ConsumableProfile {
            use_ticks_to_full: 60,
            instant_heal: 15.0,
            heal_over_time: 20.0,
            heal_duration_ticks: 200,
            use_move_multiplier: 0.5,
        }
    }

    #[test]
    fn use_fraction_ramps_from_zero_to_one_and_clamps() {
        let profile = bandage();
        assert_eq!(profile.use_fraction(0), 0.0);
        assert!((profile.use_fraction(30) - 0.5).abs() < 1e-6);
        assert_eq!(profile.use_fraction(60), 1.0);
        // Held past full: still 1.0, never overshoots.
        assert_eq!(profile.use_fraction(600), 1.0);
    }

    #[test]
    fn use_completes_only_at_full_charge() {
        let profile = bandage();
        // All-or-nothing: even one tick short does not apply. This is the rule
        // that makes an early release a free cancel.
        assert!(!profile.use_completes(0));
        assert!(!profile.use_completes(59));
        assert!(profile.use_completes(60));
        assert!(profile.use_completes(61));
    }

    #[test]
    fn heal_splits_into_an_instant_chunk_and_an_even_trickle() {
        let profile = bandage();
        assert_eq!(profile.total_heal(), 35.0);
        // 20 HP spread over 200 ticks.
        assert!((profile.heal_per_tick() - 0.1).abs() < 1e-6);
        // The trickle, summed over its whole window, is exactly the over-time
        // budget: no drift, no free health.
        let summed = profile.heal_per_tick() * profile.heal_duration_ticks as f32;
        assert!((summed - profile.heal_over_time).abs() < 1e-3);
    }

    #[test]
    fn a_zero_length_heal_window_does_not_divide_by_zero() {
        let profile = ConsumableProfile {
            heal_over_time: 20.0,
            heal_duration_ticks: 0,
            ..bandage()
        };
        assert_eq!(profile.heal_per_tick(), 0.0);
    }

    #[test]
    fn seconds_helpers_convert_through_the_tick_rate() {
        let profile = bandage();
        assert!((profile.use_seconds() - 60.0 / SERVER_TICK_RATE_HZ).abs() < 1e-6);
        assert!((profile.heal_seconds() - 200.0 / SERVER_TICK_RATE_HZ).abs() < 1e-6);
    }
}
