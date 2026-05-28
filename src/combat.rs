//! Damage primitives shared by every damage source (PvP melee today,
//! projectiles / environment later).
//!
//! The flow is the same for every kind of damage:
//!
//! 1. The source path builds a [`DamageInstance`] describing what it
//!    wants to do — raw amount, damage kind, knockback impulse magnitude,
//!    who originated it.
//! 2. The recipient's [`PlayerArmor`] is read.
//! 3. [`damage_after_armor`] reduces the raw amount.
//! 4. The reduced value is subtracted from the player's health.
//!
//! Keeping all of this off the wire shape (no `DamageInstance` ever ships
//! to a client) means new damage kinds slot in without a protocol bump.

use crate::{items::ToolKind, protocol::ClientId};

/// Category of damage. Future kinds (Pierce, Fire, Bleed, …) plug in
/// here without touching the wire protocol. Today only `Blunt` is used
/// by melee tools; `Projectile` is reserved for the upcoming bow/gun
/// pass mentioned in `docs/pvp.md` § Extensibility audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageKind {
    /// Tools, melee weapons, fists. Reduced by armor.
    Blunt,
    /// Bows, guns, thrown items (future). Reduced by armor.
    Projectile,
}

/// One self-contained damage event. Lives on the stack while the damage
/// path runs, never serialized to the wire. The client never sees the
/// raw damage value — it only sees the post-armor `damage_dealt` on
/// `ServerMessage::PlayerImpact` and the replicated `PlayerPublic.health`
/// change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageInstance {
    /// Pre-armor damage amount in HP.
    pub raw: u32,
    pub kind: DamageKind,
    /// Magnitude of the knockback impulse, in m/s. The PvP path
    /// turns this into a direction by normalising attacker → target.
    pub knockback_speed: f32,
    pub source: DamageSource,
}

/// Who originated the damage. Used so the death path can credit the
/// killer and the future hit-direction indicator can read attacker
/// position relative to the victim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageSource {
    Player { client_id: ClientId, tool: ToolKind },
}

/// Per-tool melee damage profile. The hatchet is the DPS option (short
/// swing, light knockback); the pickaxe is the burst option (long
/// swing, heavy knockback). Same trade as the gather tools express.
pub const HATCHET_PVP_DAMAGE: u32 = 8;
pub const HATCHET_KNOCKBACK_SPEED: f32 = 1.8;
pub const PICKAXE_PVP_DAMAGE: u32 = 15;
pub const PICKAXE_KNOCKBACK_SPEED: f32 = 4.0;

/// Build the PvP damage profile for one swing of `tool`. Returns `None`
/// for tool kinds that can't damage a player — Hands today, future
/// non-combat tools (a shovel, say) would also return `None` here so the
/// server can reject the swing in one branch.
pub fn tool_player_damage(tool: ToolKind, attacker: ClientId) -> Option<DamageInstance> {
    let (raw, knockback_speed) = match tool {
        ToolKind::Axe => (HATCHET_PVP_DAMAGE, HATCHET_KNOCKBACK_SPEED),
        ToolKind::Pickaxe => (PICKAXE_PVP_DAMAGE, PICKAXE_KNOCKBACK_SPEED),
        ToolKind::Hands => return None,
    };
    Some(DamageInstance {
        raw,
        kind: DamageKind::Blunt,
        knockback_speed,
        source: DamageSource::Player {
            client_id: attacker,
            tool,
        },
    })
}

/// Reduce `raw` damage by `armor` (0–100, percent). Armor is clamped
/// before the math so a future buff-stacked value of 200 doesn't roll
/// over into a healing event.
pub fn damage_after_armor(raw: u32, armor: u8) -> u32 {
    let armor = armor.min(100) as u32;
    (raw.saturating_mul(100u32.saturating_sub(armor))) / 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_reduces_damage_linearly() {
        assert_eq!(damage_after_armor(100, 0), 100);
        assert_eq!(damage_after_armor(100, 25), 75);
        assert_eq!(damage_after_armor(100, 50), 50);
        assert_eq!(damage_after_armor(100, 100), 0);
    }

    #[test]
    fn armor_clamps_above_100() {
        // A future buff-stacked armor value above 100 must still produce
        // zero damage, not wrap into negative arithmetic.
        assert_eq!(damage_after_armor(100, 250), 0);
    }

    #[test]
    fn tool_player_damage_rejects_hands() {
        assert!(tool_player_damage(ToolKind::Hands, 1).is_none());
        let axe = tool_player_damage(ToolKind::Axe, 7).expect("axe damage");
        assert_eq!(axe.raw, HATCHET_PVP_DAMAGE);
        assert!(matches!(axe.kind, DamageKind::Blunt));
        assert!(matches!(
            axe.source,
            DamageSource::Player {
                client_id: 7,
                tool: ToolKind::Axe
            }
        ));
        let pick = tool_player_damage(ToolKind::Pickaxe, 7).expect("pickaxe damage");
        assert_eq!(pick.raw, PICKAXE_PVP_DAMAGE);
    }
}
