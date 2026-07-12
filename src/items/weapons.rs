//! Weapon taxonomy: the per-item [`WeaponProfile`] that a dedicated combat
//! weapon carries on its [`ItemDefinition`], separate from the gather-oriented
//! [`ToolProfile`].
//!
//! A weapon is a pure PvP object: it declares its own damage, reach, cooldown,
//! knockback, and armor penetration, and it does not gather resources (it has no
//! `ToolProfile`, so `ToolRequirement::allows` never matches it). No weapon
//! items exist yet; this package lands the shape so Phase 2 can register melee
//! weapons and the combat path already resolves them. Every `ItemDefinition`
//! defaults `weapon: None`, so existing tool combat is untouched.

/// Combat stats for a dedicated weapon. Resolved into a
/// [`crate::combat::AttackProfile`] at the top of the server hit path (and by
/// the client's prediction), taking precedence over any [`ToolProfile`] on the
/// same item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponProfile {
    /// Raw per-swing PvP damage before armor.
    pub pvp_damage: u32,
    /// Magnitude of the knockback impulse, in m/s. Turned into a direction by
    /// the hit path (attacker -> target).
    pub knockback_speed: f32,
    /// Max feet-to-feet distance at which the swing connects, in metres.
    pub reach_m: f32,
    /// Server anti-spam floor between accepted swings, in server ticks.
    pub cooldown_ticks: u64,
    /// Percent of the target's armor this weapon ignores (0..=100). Applied
    /// before mitigation, so a 50% pierce halves the effective armor.
    pub armor_pierce_pct: u8,
    /// Impacts the weapon survives before breaking, or `None` for a weapon that
    /// never wears. Mirrors [`ToolProfile::max_durability`].
    pub max_durability: Option<u32>,
}
