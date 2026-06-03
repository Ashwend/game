//! Transient client-side combat feedback: the on-crosshair hit marker (shown
//! when the local player's swing connects) and the damage-direction arrows
//! (shown briefly toward whoever just hit the local player).
//!
//! Pure timer state with no authority of its own. The hit marker is driven by
//! the local swing prediction (`dispatch_*_swing`), the direction arrows by the
//! replicated `PlayerImpact` message on the target side. The timers are stepped
//! once per frame by `tick_combat_feedback_system` and read by the HUD.

use bevy::prelude::*;

/// How long a hit marker stays visible after a connect, in seconds.
pub(crate) const HIT_MARKER_SECONDS: f32 = 0.22;
/// How long a damage-direction arrow lingers after taking a hit, in seconds.
pub(crate) const DAMAGE_ARROW_SECONDS: f32 = 1.6;
/// Cap on concurrent direction arrows so a burst of hits can't grow the vec
/// without bound; the oldest is dropped once this is exceeded.
const MAX_DAMAGE_ARROWS: usize = 6;

/// One "damage came from here" marker. `source` is the attacker's world
/// position at impact time; `remaining` counts down to zero and drives the fade.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DamageArrow {
    pub(crate) source: Vec3,
    remaining: f32,
    total: f32,
}

impl DamageArrow {
    /// 1.0 at spawn, 0.0 at expiry. Used as the arrow's alpha multiplier.
    pub(crate) fn fade(&self) -> f32 {
        if self.total <= 0.0 {
            return 0.0;
        }
        (self.remaining / self.total).clamp(0.0, 1.0)
    }
}

#[derive(Resource, Debug, Default)]
pub(crate) struct CombatFeedbackState {
    hit_marker_remaining: f32,
    /// True when the most recent hit landed on a player, which tints the marker
    /// hotter so a PvP connect reads differently from a world hit.
    hit_marker_player: bool,
    damage_arrows: Vec<DamageArrow>,
}

impl CombatFeedbackState {
    /// Record that the local player's swing just connected. `is_player` tints
    /// the marker for PvP hits.
    pub(crate) fn trigger_hit_marker(&mut self, is_player: bool) {
        self.hit_marker_remaining = HIT_MARKER_SECONDS;
        self.hit_marker_player = is_player;
    }

    /// Record that the local player just took a hit from `source` (the
    /// attacker's world position). Adds a fading directional arrow.
    pub(crate) fn push_damage_from(&mut self, source: Vec3) {
        self.damage_arrows.push(DamageArrow {
            source,
            remaining: DAMAGE_ARROW_SECONDS,
            total: DAMAGE_ARROW_SECONDS,
        });
        if self.damage_arrows.len() > MAX_DAMAGE_ARROWS {
            self.damage_arrows.remove(0);
        }
    }

    /// Advance every timer by `dt` and drop expired arrows.
    pub(crate) fn advance(&mut self, dt: f32) {
        let dt = dt.max(0.0);
        self.hit_marker_remaining = (self.hit_marker_remaining - dt).max(0.0);
        if self.hit_marker_remaining == 0.0 {
            self.hit_marker_player = false;
        }
        for arrow in &mut self.damage_arrows {
            arrow.remaining -= dt;
        }
        self.damage_arrows.retain(|arrow| arrow.remaining > 0.0);
    }

    /// Hit-marker fade in `[0.0, 1.0]`, 0 when inactive.
    pub(crate) fn hit_marker_fade(&self) -> f32 {
        (self.hit_marker_remaining / HIT_MARKER_SECONDS).clamp(0.0, 1.0)
    }

    pub(crate) fn hit_marker_is_player(&self) -> bool {
        self.hit_marker_player
    }

    pub(crate) fn damage_arrows(&self) -> &[DamageArrow] {
        &self.damage_arrows
    }

    /// Whether anything is currently on-screen, so the HUD can request a
    /// repaint to keep the fades animating without a continuous redraw.
    pub(crate) fn is_active(&self) -> bool {
        self.hit_marker_remaining > 0.0 || !self.damage_arrows.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_marker_fades_out_over_its_lifetime() {
        let mut state = CombatFeedbackState::default();
        assert_eq!(state.hit_marker_fade(), 0.0);

        state.trigger_hit_marker(true);
        assert!(state.hit_marker_fade() > 0.99);
        assert!(state.hit_marker_is_player());

        state.advance(HIT_MARKER_SECONDS * 0.5);
        let mid = state.hit_marker_fade();
        assert!(mid > 0.0 && mid < 1.0);

        state.advance(HIT_MARKER_SECONDS);
        assert_eq!(state.hit_marker_fade(), 0.0);
        assert!(!state.hit_marker_is_player());
    }

    #[test]
    fn damage_arrows_expire_and_are_capped() {
        let mut state = CombatFeedbackState::default();
        for i in 0..(MAX_DAMAGE_ARROWS + 3) {
            state.push_damage_from(Vec3::new(i as f32, 0.0, 0.0));
        }
        assert_eq!(state.damage_arrows().len(), MAX_DAMAGE_ARROWS);

        state.advance(DAMAGE_ARROW_SECONDS + 0.1);
        assert!(state.damage_arrows().is_empty());
        assert!(!state.is_active());
    }
}
