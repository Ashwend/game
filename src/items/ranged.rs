//! Ranged-weapon taxonomy: the per-item [`RangedProfile`] a bow or crossbow
//! carries on its [`ItemDefinition`](crate::items::ItemDefinition), parallel
//! to the melee [`WeaponProfile`](crate::items::WeaponProfile).
//!
//! A ranged weapon does not swing: it draws (bow) or reloads (crossbow) and then
//! fires a server-simulated projectile. The profile declares the shot's damage
//! band, projectile speed, draw window, cooldown, and which item id it consumes
//! as ammo. Damage scales with observed draw fraction on the server (a crossbow
//! has `draw_ticks_to_full == 0`, so it is always full damage but gated by
//! `cooldown_ticks`). Like tools and melee weapons, a ranged weapon has no
//! [`crate::items::ToolProfile`], so it gathers nothing.

/// Combat + ballistic stats for a dedicated ranged weapon (bow, crossbow).
/// Consumed by the server fire path (`ClientMessage::Ranged`) to validate the
/// shot, compute its scaled damage, and launch the projectile. Every field is
/// authoritative: the client never sends damage or projectile velocity, only the
/// aim direction and the draw/fire intent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangedProfile {
    /// Minimum shot damage, dealt on an instant (undrawn) release. The floor of
    /// the draw-scaled damage band.
    pub damage_min: u32,
    /// Maximum shot damage, dealt at full draw. Damage lerps from `damage_min`
    /// to this across `draw_ticks_to_full` ticks of held draw.
    pub damage_max: u32,
    /// Projectile launch speed, in metres per second. The server multiplies the
    /// aim unit vector by this to seed the projectile velocity.
    pub projectile_speed_mps: f32,
    /// Ticks of held draw at which damage reaches `damage_max`. `0` disables draw
    /// scaling: the shot is always `damage_max` (the crossbow), gated only by the
    /// reload `cooldown_ticks`.
    pub draw_ticks_to_full: u64,
    /// Server anti-spam / reload floor between accepted shots, in ticks. For the
    /// crossbow this is the reload window; for the bow it is a small post-fire
    /// floor since the draw time is the real pacing lever.
    pub cooldown_ticks: u64,
    /// Item id this weapon consumes per shot. One is taken from the shooter's
    /// inventory on a valid fire; a shot with no matching ammo is rejected.
    pub ammo_item: &'static str,
    /// Magnitude of the knockback impulse a hit applies, in m/s. Turned into a
    /// direction (shooter -> target) by the shared post-hit tail.
    pub knockback_speed: f32,
    /// Impacts the weapon survives before breaking, or `None` for a weapon that
    /// never wears. Mirrors [`crate::items::WeaponProfile::max_durability`].
    pub max_durability: Option<u32>,
}

impl RangedProfile {
    /// Observed draw fraction in `[0, 1]` after `draw_ticks` ticks of held draw.
    /// `1` for an instant-fire weapon (no draw window to scale against).
    pub fn draw_fraction(&self, draw_ticks: u64) -> f32 {
        if self.draw_ticks_to_full == 0 {
            return 1.0;
        }
        (draw_ticks as f32 / self.draw_ticks_to_full as f32).clamp(0.0, 1.0)
    }

    /// Damage for a shot released after `draw_ticks` ticks of held draw, lerped
    /// linearly from `damage_min` (instant release) to `damage_max` (full draw).
    ///
    /// A weapon with `draw_ticks_to_full == 0` (the crossbow) is always
    /// `damage_max`: there is no draw to scale, and the reload cooldown is what
    /// gates its rate of fire. The draw fraction is clamped to `[0, 1]` so a shot
    /// held past full draw does not overshoot the ceiling.
    pub fn damage_for_draw(&self, draw_ticks: u64) -> u32 {
        let fraction = self.draw_fraction(draw_ticks);
        let min = self.damage_min as f32;
        let max = self.damage_max as f32;
        (min + (max - min) * fraction).round() as u32
    }

    /// Launch speed for a draw fraction in `[0, 1]`: lerped from the
    /// [`crate::game_balance::BOW_MIN_RELEASE_SPEED_FRACTION`] floor of the full
    /// `projectile_speed_mps` up to the full speed at full draw, so the shot's
    /// pace (and with it range and drop) follows the hold. Instant-fire weapons
    /// (crossbow) always launch at full speed: the prod is mechanically drawn.
    /// Shared by the server's fire path (tick-observed fraction) and the
    /// client's own-arrow prediction (its local fraction), so both integrate
    /// the same arc.
    pub fn speed_for_draw_fraction(&self, fraction: f32) -> f32 {
        if self.draw_ticks_to_full == 0 {
            return self.projectile_speed_mps;
        }
        let floor = crate::game_balance::BOW_MIN_RELEASE_SPEED_FRACTION;
        self.projectile_speed_mps * (floor + (1.0 - floor) * fraction.clamp(0.0, 1.0))
    }

    /// Launch speed for a shot released after `draw_ticks` ticks of held draw.
    pub fn speed_for_draw(&self, draw_ticks: u64) -> f32 {
        self.speed_for_draw_fraction(self.draw_fraction(draw_ticks))
    }

    /// Whether a release after `draw_ticks` of held draw fires at all. A draw
    /// weapon must be held to at least
    /// [`crate::game_balance::BOW_MIN_DRAW_FRACTION_TO_FIRE`] of full draw; a
    /// shorter release is a cancel, never a tap-shot. Instant-fire weapons
    /// always fire (their pacing is the reload cooldown).
    pub fn draw_fires(&self, draw_ticks: u64) -> bool {
        self.draw_ticks_to_full == 0
            || self.draw_fraction(draw_ticks) >= crate::game_balance::BOW_MIN_DRAW_FRACTION_TO_FIRE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bow() -> RangedProfile {
        RangedProfile {
            damage_min: 15,
            damage_max: 40,
            projectile_speed_mps: 35.0,
            draw_ticks_to_full: 30,
            cooldown_ticks: 5,
            ammo_item: "arrow",
            knockback_speed: 1.0,
            max_durability: Some(200),
        }
    }

    fn crossbow() -> RangedProfile {
        RangedProfile {
            damage_min: 55,
            damage_max: 55,
            projectile_speed_mps: 55.0,
            draw_ticks_to_full: 0,
            cooldown_ticks: 70,
            ammo_item: "arrow",
            knockback_speed: 1.5,
            max_durability: Some(600),
        }
    }

    #[test]
    fn instant_release_deals_the_minimum() {
        assert_eq!(bow().damage_for_draw(0), 15);
    }

    #[test]
    fn full_draw_deals_the_maximum() {
        assert_eq!(bow().damage_for_draw(30), 40);
    }

    #[test]
    fn overdraw_clamps_at_the_maximum() {
        // Holding past full draw never overshoots the ceiling.
        assert_eq!(bow().damage_for_draw(1000), 40);
    }

    #[test]
    fn half_draw_lands_midway() {
        // 15 ticks of a 30-tick draw is 50% => halfway from 15 to 40 = 27.5,
        // rounded to 28.
        assert_eq!(bow().damage_for_draw(15), 28);
    }

    #[test]
    fn crossbow_ignores_draw_and_stays_flat() {
        // A zero draw window means every shot is the flat max, regardless of any
        // observed draw ticks.
        assert_eq!(crossbow().damage_for_draw(0), 55);
        assert_eq!(crossbow().damage_for_draw(100), 55);
    }

    #[test]
    fn launch_speed_follows_the_draw() {
        let bow = bow();
        let full = bow.speed_for_draw(30);
        let floor = bow.speed_for_draw(0);
        let half = bow.speed_for_draw(15);
        assert!(
            (full - bow.projectile_speed_mps).abs() < 1e-4,
            "full draw launches at the profile's full speed"
        );
        assert!(
            floor < full * 0.6,
            "an instant release launches well under full pace: {floor} vs {full}"
        );
        assert!(
            floor < half && half < full,
            "speed scales monotonically with the hold"
        );
        // Overdraw clamps like damage does.
        assert!((bow.speed_for_draw(1000) - full).abs() < 1e-4);
    }

    #[test]
    fn crossbow_always_launches_at_full_speed() {
        let crossbow = crossbow();
        assert_eq!(crossbow.speed_for_draw(0), crossbow.projectile_speed_mps);
    }

    #[test]
    fn a_too_short_draw_never_fires_but_a_committed_one_does() {
        let bow = bow();
        assert!(!bow.draw_fires(0), "a tap can never loose an arrow");
        assert!(
            !bow.draw_fires(3),
            "releasing below the minimum draw is a cancel"
        );
        assert!(bow.draw_fires(8), "a quarter draw (and past) fires");
        assert!(bow.draw_fires(30), "a full draw fires");
        assert!(
            crossbow().draw_fires(0),
            "an instant-fire weapon has no draw gate"
        );
    }
}
