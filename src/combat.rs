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
    protocol::{ClientId, Vec3Net},
};

use crate::game_balance::{
    COMBAT_PLAYER_BODY_CENTRE_Y, COMBAT_PLAYER_BODY_HALF_HEIGHT, COMBAT_PLAYER_BODY_HALF_WIDTH,
    COMBAT_SLEEPING_BODY_CENTRE_Y, COMBAT_SLEEPING_BODY_HALF_HEIGHT,
    COMBAT_SLEEPING_BODY_HALF_WIDTH,
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

/// Distance along the look ray at which it enters a player's body box, or `None`
/// if the ray misses.
///
/// `eye` is the attacker's eye position, `forward` the **normalised** look
/// direction (`items::look_forward(yaw, pitch)`), `target_feet` the target's
/// ground position, and `sleeping` selects the low/wide logged-out-body box vs
/// the upright standing column.
///
/// This is the single shared melee-aim test: the client calls it to decide
/// which player a swing targets (and to predict the impact), and the server
/// calls it to validate the incoming `AttackPlayer`. Because both sides test
/// the *same* volume with the *same* slab-method ray-AABB, "my crosshair was on
/// them" and "the server accepted the hit" can't disagree. It replaces an older
/// server-only cone-to-a-chest-point test that rejected point-blank hits: at
/// close range the eye sits well above the chest point, so the eye→chest vector
/// tilts steeply down and fell outside the cone even with the crosshair dead on
/// the target, the attacker saw predicted feedback while the victim took no hit.
pub fn player_body_ray_entry(
    eye: Vec3Net,
    forward: Vec3Net,
    target_feet: Vec3Net,
    sleeping: bool,
) -> Option<f32> {
    let (centre_y, half_width, half_height) = if sleeping {
        (
            COMBAT_SLEEPING_BODY_CENTRE_Y,
            COMBAT_SLEEPING_BODY_HALF_WIDTH,
            COMBAT_SLEEPING_BODY_HALF_HEIGHT,
        )
    } else {
        (
            COMBAT_PLAYER_BODY_CENTRE_Y,
            COMBAT_PLAYER_BODY_HALF_WIDTH,
            COMBAT_PLAYER_BODY_HALF_HEIGHT,
        )
    };
    let min = Vec3Net::new(
        target_feet.x - half_width,
        target_feet.y + centre_y - half_height,
        target_feet.z - half_width,
    );
    let max = Vec3Net::new(
        target_feet.x + half_width,
        target_feet.y + centre_y + half_height,
        target_feet.z + half_width,
    );

    let mut t_near: f32 = f32::NEG_INFINITY;
    let mut t_far: f32 = f32::INFINITY;
    for axis in 0..3 {
        let (o, d, mn, mx) = match axis {
            0 => (eye.x, forward.x, min.x, max.x),
            1 => (eye.y, forward.y, min.y, max.y),
            _ => (eye.z, forward.z, min.z, max.z),
        };
        if d.abs() < 1e-6 {
            // Ray parallel to this slab: a miss unless the origin is already
            // between the slab planes.
            if o < mn || o > mx {
                return None;
            }
            continue;
        }
        let inv_d = d.recip();
        let mut t1 = (mn - o) * inv_d;
        let mut t2 = (mx - o) * inv_d;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_near = t_near.max(t1);
        t_far = t_far.min(t2);
        if t_near > t_far {
            return None;
        }
    }
    if t_far < 0.0 {
        // Box entirely behind the eye.
        return None;
    }
    // Inside the box clamps to 0 so a point-blank poke still resolves as a hit.
    Some(t_near.max(0.0))
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

    // Eye height the look ray originates from in these tests; mirrors
    // COMBAT_ATTACKER_EYE_HEIGHT without depending on it.
    const EYE: f32 = 1.62;

    #[test]
    fn body_ray_hits_target_in_front() {
        // Looking straight along -Z (yaw=0,pitch=0) at a target 2 m ahead.
        let eye = Vec3Net::new(0.0, EYE, 0.0);
        let forward = crate::items::look_forward(0.0, 0.0);
        let target = Vec3Net::new(0.0, 0.0, -2.0);
        let distance = player_body_ray_entry(eye, forward, target, false).expect("hit");
        assert!(
            distance > 0.0 && distance < 2.5,
            "entry distance {distance}"
        );
    }

    #[test]
    fn body_ray_hits_point_blank_target_with_level_aim() {
        // Regression for the "too close" bug: a target half a metre ahead, eye
        // level (pitch 0). The eye (1.62) is well above the chest, so the old
        // eye->chest cone test rejected this. A level look ray still passes
        // through the standing body box (which spans y≈0..1.9), so it must hit.
        let eye = Vec3Net::new(0.0, EYE, 0.0);
        let forward = crate::items::look_forward(0.0, 0.0);
        let target = Vec3Net::new(0.0, 0.0, -0.5);
        assert!(
            player_body_ray_entry(eye, forward, target, false).is_some(),
            "point-blank level-aim swing must register"
        );
    }

    #[test]
    fn body_ray_misses_target_behind() {
        let eye = Vec3Net::new(0.0, EYE, 0.0);
        let forward = crate::items::look_forward(0.0, 0.0); // facing -Z
        let target = Vec3Net::new(0.0, 0.0, 2.0); // behind, at +Z
        assert!(player_body_ray_entry(eye, forward, target, false).is_none());
    }

    #[test]
    fn body_ray_misses_target_off_to_the_side() {
        let eye = Vec3Net::new(0.0, EYE, 0.0);
        let forward = crate::items::look_forward(0.0, 0.0); // facing -Z
        // 2 m ahead but 1.5 m to the side, well outside the 0.4 m-wide box.
        let target = Vec3Net::new(1.5, 0.0, -2.0);
        assert!(player_body_ray_entry(eye, forward, target, false).is_none());
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
