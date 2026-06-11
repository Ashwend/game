//! Damage primitives shared by every damage source (PvP melee today,
//! projectiles / environment later).
//!
//! The flow is the same for every kind of damage:
//!
//! 1. The source path builds a [`DamageInstance`] describing what it
//!    wants to do, raw amount, damage kind, knockback impulse magnitude,
//!    who originated it.
//! 2. The recipient's [`PlayerArmor`] is read.
//! 3. [`damage_after_armor`] reduces the raw amount.
//! 4. The reduced value is subtracted from the player's health.
//!
//! Keeping all of this off the wire shape (no `DamageInstance` ever ships
//! to a client) means new damage kinds slot in without a protocol bump.

use crate::{
    items::{ToolKind, ToolProfile},
    protocol::ClientId,
};

pub(crate) use crate::game_balance::{HATCHET_KNOCKBACK_SPEED, PICKAXE_KNOCKBACK_SPEED};

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
/// raw damage value, it only sees the post-armor `damage_dealt` on
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

/// Build the PvP damage profile for one swing of `tool`. The damage
/// amount comes from the tool's own profile so higher tiers hit harder
/// (an iron pickaxe outdamages a stone one); knockback stays a
/// kind-level trait, the hatchet is the DPS option (short swing, light
/// knockback) and the pickaxe the burst option (long swing, heavy
/// knockback). Returns `None` for tools that can't damage a player,
/// Hands today; future non-combat tools (a shovel, say) declare
/// `player_damage: 0` and get rejected in the same branch.
pub fn tool_player_damage(tool: ToolProfile, attacker: ClientId) -> Option<DamageInstance> {
    let knockback_speed = match tool.kind {
        ToolKind::Axe => HATCHET_KNOCKBACK_SPEED,
        ToolKind::Pickaxe => PICKAXE_KNOCKBACK_SPEED,
        // Hands and the hammer can't damage players; the hammer is a
        // construction tool, not a weapon.
        ToolKind::Hands | ToolKind::Hammer => return None,
    };
    if tool.player_damage == 0 {
        return None;
    }
    Some(DamageInstance {
        raw: tool.player_damage,
        kind: DamageKind::Blunt,
        knockback_speed,
        source: DamageSource::Player {
            client_id: attacker,
            tool: tool.kind,
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

    fn registered_tool(item_id: &str) -> ToolProfile {
        crate::items::item_definition(item_id)
            .and_then(|definition| definition.tool)
            .expect("registered tool profile")
    }

    #[test]
    fn tool_player_damage_rejects_hands() {
        assert!(tool_player_damage(crate::items::HANDS_TOOL, 1).is_none());
        let axe = tool_player_damage(registered_tool(crate::items::BASIC_HATCHET_ID), 7)
            .expect("axe damage");
        assert_eq!(axe.raw, crate::game_balance::STONE_HATCHET_PVP_DAMAGE);
        assert!(matches!(axe.kind, DamageKind::Blunt));
        assert!(matches!(
            axe.source,
            DamageSource::Player {
                client_id: 7,
                tool: ToolKind::Axe
            }
        ));
        let pick = tool_player_damage(registered_tool(crate::items::BASIC_PICKAXE_ID), 7)
            .expect("pickaxe damage");
        assert_eq!(pick.raw, crate::game_balance::STONE_PICKAXE_PVP_DAMAGE);
    }

    #[test]
    fn iron_tools_outdamage_their_stone_counterparts() {
        let stone_axe = tool_player_damage(registered_tool(crate::items::BASIC_HATCHET_ID), 1)
            .expect("stone axe damage");
        let iron_axe = tool_player_damage(registered_tool(crate::items::IRON_HATCHET_ID), 1)
            .expect("iron axe damage");
        assert!(iron_axe.raw > stone_axe.raw);
        // Knockback is a kind trait, not a tier trait: upgrading the tool
        // changes damage, not the shove.
        assert_eq!(iron_axe.knockback_speed, stone_axe.knockback_speed);

        let stone_pick = tool_player_damage(registered_tool(crate::items::BASIC_PICKAXE_ID), 1)
            .expect("stone pickaxe damage");
        let iron_pick = tool_player_damage(registered_tool(crate::items::IRON_PICKAXE_ID), 1)
            .expect("iron pickaxe damage");
        assert!(iron_pick.raw > stone_pick.raw);
    }
}
