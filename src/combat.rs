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
    items::{ItemDefinition, ItemModel, ToolKind, ToolProfile, WeaponProfile},
    protocol::{ClientId, Vec3Net},
};

pub(crate) use crate::game_balance::{
    COMBAT_ATTACK_RANGE_M, HATCHET_KNOCKBACK_SPEED, PICKAXE_KNOCKBACK_SPEED,
};
use crate::game_balance::{
    COMBAT_PLAYER_BODY_CENTRE_Y, COMBAT_PLAYER_BODY_HALF_HEIGHT, COMBAT_PLAYER_BODY_HALF_WIDTH,
    COMBAT_SLEEPING_BODY_CENTRE_Y, COMBAT_SLEEPING_BODY_HALF_HEIGHT,
    COMBAT_SLEEPING_BODY_HALF_WIDTH,
};

/// Category of damage. Future kinds (Pierce, Fire, Bleed, …) plug in
/// here without touching the wire protocol. Today only `Blunt` is used
/// by melee tools; `Projectile` is reserved for the upcoming bow/gun
/// pass mentioned in `docs/pvp-combat.md` § Extensibility audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageKind {
    /// Tools, melee weapons, fists. Reduced by armor.
    Blunt,
    /// Bows, guns, thrown items (future). Reduced by armor.
    Projectile,
    /// Explosions (future Phase 6). Reduced by armor. Unused today; the
    /// variant is declared now so the blast mitigation slot exists before the
    /// explosive pass lands.
    Blast,
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
    /// A PvP melee hit. `model` is the swing's peer-visible impact identity
    /// (the weapon's own archetype, or a gather tool's archetype), the same
    /// value that rides `ServerMessage::PlayerImpact` so a peer's audio, VFX,
    /// and camera reaction key on what actually landed the hit.
    Player {
        client_id: ClientId,
        model: ItemModel,
    },
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
            model: tool.kind.swing_model(),
        },
    })
}

/// Resolved combat stats for one swing, independent of whether the swing came
/// from a gather tool or a dedicated weapon. Built once at the top of the hit
/// path (server) and by the client's swing prediction, so both sides read the
/// same reach, cooldown, and pierce off the same resolution rule and can never
/// disagree about whether a hit was in range.
///
/// `model` carries the swing's peer-visible impact identity for the wire shapes
/// (`ServerMessage::PlayerImpact` and [`DamageSource::Player`]): a dedicated
/// weapon resolves to its own archetype (Club/Spear/Sword/Mace), a gather tool
/// to its archetype (Hatchet/Pickaxe). This is what a peer's audio, VFX, and
/// camera reaction key on, so a mace hit reads as a mace even to observers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackProfile {
    /// Raw pre-armor damage in HP.
    pub damage: u32,
    pub kind: DamageKind,
    /// Knockback impulse magnitude in m/s.
    pub knockback_speed: f32,
    /// Max feet-to-feet distance the swing reaches, in metres.
    pub reach_m: f32,
    /// Server anti-spam floor between accepted swings, in ticks.
    pub cooldown_ticks: u64,
    /// Percent of the target's armor ignored before mitigation (0..=100).
    pub armor_pierce_pct: u8,
    /// The swing's peer-visible impact identity (weapon's own archetype, or a
    /// gather tool's archetype). Always present, bare hands and the hammer never
    /// resolve to an `AttackProfile` at all, so there is no empty-hand case here.
    pub model: ItemModel,
}

/// Resolve the [`AttackProfile`] for one swing of `definition`, or `None` when
/// the item can't damage a player (bare hands, the hammer, or a definition with
/// neither a weapon nor a combat tool). A [`WeaponProfile`] takes precedence
/// over a [`ToolProfile`] on the same item; tools produce exactly today's
/// numbers so existing combat is byte-identical. The impact `model` comes from
/// the definition's own archetype (a weapon its own, a tool its Hatchet/Pickaxe).
pub fn resolve_attack_profile(definition: &ItemDefinition) -> Option<AttackProfile> {
    resolve_attack_profile_parts(definition.model, definition.weapon, definition.tool)
}

/// The precedence rule behind [`resolve_attack_profile`], taking the impact
/// `model` and the two optional profiles directly so the hands fallback (which
/// has a bare [`ToolProfile`] and no [`ItemDefinition`]) and unit tests can
/// reuse it. Weapon first, then tool; `None` when neither can damage a player.
/// For a gather tool the `model` argument is ignored in favour of the tool's own
/// archetype ([`ToolKind::swing_model`]), so the wire identity always matches the
/// authoritative tool even if a caller passed a stale registry model.
pub fn resolve_attack_profile_parts(
    model: ItemModel,
    weapon: Option<WeaponProfile>,
    tool: Option<ToolProfile>,
) -> Option<AttackProfile> {
    if let Some(weapon) = weapon {
        return Some(AttackProfile {
            damage: weapon.pvp_damage,
            kind: DamageKind::Blunt,
            knockback_speed: weapon.knockback_speed,
            reach_m: weapon.reach_m,
            cooldown_ticks: weapon.cooldown_ticks,
            armor_pierce_pct: weapon.armor_pierce_pct,
            // A weapon's impact identity is its own registry archetype
            // (Club/Spear/Sword/Mace).
            model,
        });
    }
    tool.and_then(attack_profile_from_tool)
}

/// Resolve an [`AttackProfile`] from a gather tool, reproducing today's exact
/// PvP numbers: the knockback per kind and the damage per tier come from
/// [`tool_player_damage`]'s tables, the cooldown from the tool's own
/// `cooldown_ticks`, reach from [`COMBAT_ATTACK_RANGE_M`], and zero pierce.
/// `None` for bare hands and the hammer (same short-circuit as
/// `tool_player_damage`). The impact identity is the tool's own archetype so a
/// hatchet reads as a hatchet on the wire regardless of the registry `model`.
fn attack_profile_from_tool(tool: ToolProfile) -> Option<AttackProfile> {
    // Route through the existing table so the damage/knockback stay a single
    // source of truth. The client id here is a throwaway: `AttackProfile`
    // doesn't carry a `DamageSource`, only the numbers.
    let instance = tool_player_damage(tool, 0)?;
    Some(AttackProfile {
        damage: instance.raw,
        kind: instance.kind,
        knockback_speed: instance.knockback_speed,
        reach_m: COMBAT_ATTACK_RANGE_M,
        cooldown_ticks: tool.cooldown_ticks,
        armor_pierce_pct: 0,
        model: tool.kind.swing_model(),
    })
}

/// Effective armor after a weapon's pierce is applied, capped at 100. Pierce
/// runs before mitigation: `effective = armor * (100 - pierce) / 100`, so 50%
/// pierce against 40 armor leaves 20 effective armor. Saturating throughout so
/// a future over-100 armor value can't wrap.
pub fn effective_armor_after_pierce(armor: u8, armor_pierce_pct: u8) -> u8 {
    let armor = armor.min(100) as u32;
    let pierce = armor_pierce_pct.min(100) as u32;
    let remaining = 100u32.saturating_sub(pierce);
    ((armor.saturating_mul(remaining)) / 100).min(100) as u8
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
                model: ItemModel::Hatchet
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

    // ---- AttackProfile resolution ----

    use crate::items::{HANDS_TOOL, HeldMesh, ItemDefinition, ItemModel, ItemTint};

    /// A bare-bones registry-shaped `ItemDefinition` for resolution tests. All
    /// the cosmetic/registry fields are filler; only `tool`/`weapon` matter.
    fn definition_with(tool: Option<ToolProfile>, weapon: Option<WeaponProfile>) -> ItemDefinition {
        ItemDefinition {
            id: "test_item",
            name: "Test Item",
            description: "",
            stack_size: 1,
            equipable: true,
            model: ItemModel::Bag,
            held_mesh: HeldMesh::Bag,
            tint: ItemTint::new(0, 0, 0),
            tool,
            weapon,
            ranged: None,
            armor: None,
            explosive: None,
            deployable: None,
        }
    }

    fn sample_tool(kind: ToolKind, damage: u32, cooldown: u64) -> ToolProfile {
        ToolProfile {
            kind,
            tier: 1,
            gather_amount: 6,
            cooldown_ticks: cooldown,
            max_durability: Some(100),
            player_damage: damage,
        }
    }

    #[test]
    fn resolve_attack_profile_tools_reproduce_todays_numbers() {
        // A registered stone hatchet must resolve to exactly the values the old
        // `tool_player_damage` path produced: today's damage, today's kind,
        // today's knockback, reach from COMBAT_ATTACK_RANGE_M, zero pierce,
        // cooldown from the tool, and a Some(tool) impact identity.
        let definition = crate::items::item_definition(crate::items::BASIC_HATCHET_ID)
            .expect("stone hatchet registered");
        let tool = definition.tool.expect("hatchet has a tool profile");
        let profile = resolve_attack_profile(definition).expect("hatchet resolves");
        assert_eq!(profile.damage, tool.player_damage);
        assert_eq!(
            profile.damage,
            crate::game_balance::STONE_HATCHET_PVP_DAMAGE
        );
        assert!(matches!(profile.kind, DamageKind::Blunt));
        assert_eq!(profile.knockback_speed, HATCHET_KNOCKBACK_SPEED);
        assert_eq!(profile.reach_m, COMBAT_ATTACK_RANGE_M);
        assert_eq!(profile.cooldown_ticks, tool.cooldown_ticks);
        assert_eq!(profile.armor_pierce_pct, 0);
        // A gather tool's wire identity is its own archetype (a hatchet reads
        // as a hatchet), independent of any registry model passed in.
        assert_eq!(profile.model, ItemModel::Hatchet);
    }

    #[test]
    fn resolve_attack_profile_hands_and_hammer_resolve_to_none() {
        // Bare hands (the synthesized HANDS_TOOL) and the hammer both short
        // circuit to None, exactly as `tool_player_damage` does, so a swing
        // with either never reaches the damage path.
        assert!(resolve_attack_profile_parts(ItemModel::Bag, None, Some(HANDS_TOOL)).is_none());

        let hammer_definition =
            crate::items::item_definition(crate::items::HAMMER_ID).expect("hammer registered");
        assert!(hammer_definition.weapon.is_none());
        assert!(resolve_attack_profile(hammer_definition).is_none());

        // A definition with neither a weapon nor a combat tool resolves to
        // None too.
        assert!(resolve_attack_profile(&definition_with(None, None)).is_none());
    }

    #[test]
    fn resolve_attack_profile_weapon_beats_tool() {
        // When an item carries both a weapon and a tool profile, the weapon
        // wins: its damage/reach/cooldown/pierce are used and the impact identity
        // is the weapon's own archetype (the `model` argument), not the tool's.
        let weapon = WeaponProfile {
            pvp_damage: 40,
            knockback_speed: 6.5,
            reach_m: 4.25,
            cooldown_ticks: 9,
            armor_pierce_pct: 30,
            max_durability: Some(250),
        };
        let tool = sample_tool(ToolKind::Axe, 8, 6);
        let profile = resolve_attack_profile_parts(ItemModel::Sword, Some(weapon), Some(tool))
            .expect("weapon-and-tool resolves");
        assert_eq!(profile.damage, 40);
        assert_eq!(profile.knockback_speed, 6.5);
        assert_eq!(profile.reach_m, 4.25);
        assert_eq!(profile.cooldown_ticks, 9);
        assert_eq!(profile.armor_pierce_pct, 30);
        // The weapon's impact identity is its own archetype, not the tool's.
        assert_eq!(profile.model, ItemModel::Sword);
    }

    #[test]
    fn resolve_attack_profile_weapon_only_uses_profile_reach_and_cooldown() {
        // A synthetic definition with ONLY a WeaponProfile (no tool) resolves,
        // reading reach and cooldown straight from the profile rather than the
        // COMBAT_ATTACK_RANGE_M tool fallback.
        let weapon = WeaponProfile {
            pvp_damage: 55,
            knockback_speed: 3.0,
            reach_m: 5.0,
            cooldown_ticks: 12,
            armor_pierce_pct: 0,
            max_durability: None,
        };
        let definition = definition_with(None, Some(weapon));
        let profile = resolve_attack_profile(&definition).expect("weapon-only resolves");
        assert_eq!(profile.damage, 55);
        assert_eq!(profile.reach_m, 5.0);
        assert_ne!(profile.reach_m, COMBAT_ATTACK_RANGE_M);
        assert_eq!(profile.cooldown_ticks, 12);
        // `definition_with` builds a Bag-model definition, so the weapon's wire
        // identity is that model.
        assert_eq!(profile.model, ItemModel::Bag);
    }

    // ---- weapon resolution ----

    /// Resolve the `AttackProfile` for a registered weapon by id. Panics if the
    /// item is missing or resolves to no profile, so a broken registry row is a
    /// test failure rather than a silent `None`.
    fn weapon_profile(item_id: &str) -> AttackProfile {
        let definition = crate::items::item_definition(item_id).expect("weapon registered");
        assert!(
            definition.weapon.is_some(),
            "{item_id} should carry a WeaponProfile"
        );
        assert!(
            definition.tool.is_none(),
            "{item_id} is a weapon, it must gather nothing"
        );
        resolve_attack_profile(definition).expect("weapon resolves to an AttackProfile")
    }

    #[test]
    fn each_melee_weapon_resolves_to_its_game_balance_values() {
        use crate::game_balance::{
            COMBAT_ATTACK_RANGE_M, IRON_MACE_ARMOR_PIERCE_PCT, IRON_MACE_COOLDOWN_TICKS,
            IRON_MACE_KNOCKBACK_SPEED, IRON_MACE_PVP_DAMAGE, IRON_SWORD_COOLDOWN_TICKS,
            IRON_SWORD_KNOCKBACK_SPEED, IRON_SWORD_PVP_DAMAGE, STONE_SPEAR_COOLDOWN_TICKS,
            STONE_SPEAR_KNOCKBACK_SPEED, STONE_SPEAR_PVP_DAMAGE, STONE_SPEAR_REACH_M,
            WOODEN_CLUB_COOLDOWN_TICKS, WOODEN_CLUB_KNOCKBACK_SPEED, WOODEN_CLUB_PVP_DAMAGE,
        };
        use crate::items::{
            IRON_MACE_ID, IRON_SWORD_ID, ItemModel, STONE_SPEAR_ID, WOODEN_CLUB_ID,
        };

        // Every weapon resolves with Blunt damage, its own peer-visible impact
        // model (never a gather-tool archetype), and exactly its game_balance
        // numbers. Reach is the melee default except the spear; pierce is zero
        // except the mace.
        let club = weapon_profile(WOODEN_CLUB_ID);
        assert_eq!(club.damage, WOODEN_CLUB_PVP_DAMAGE);
        assert_eq!(club.knockback_speed, WOODEN_CLUB_KNOCKBACK_SPEED);
        assert_eq!(club.reach_m, COMBAT_ATTACK_RANGE_M);
        assert_eq!(club.cooldown_ticks, WOODEN_CLUB_COOLDOWN_TICKS);
        assert_eq!(club.armor_pierce_pct, 0);
        assert!(matches!(club.kind, DamageKind::Blunt));
        assert_eq!(club.model, ItemModel::Club);

        let spear = weapon_profile(STONE_SPEAR_ID);
        assert_eq!(spear.damage, STONE_SPEAR_PVP_DAMAGE);
        assert_eq!(spear.knockback_speed, STONE_SPEAR_KNOCKBACK_SPEED);
        assert_eq!(spear.reach_m, STONE_SPEAR_REACH_M);
        assert!(
            spear.reach_m > COMBAT_ATTACK_RANGE_M,
            "the spear reaches past standard melee"
        );
        assert_eq!(spear.cooldown_ticks, STONE_SPEAR_COOLDOWN_TICKS);
        assert_eq!(spear.armor_pierce_pct, 0);
        assert_eq!(spear.model, ItemModel::Spear);

        let sword = weapon_profile(IRON_SWORD_ID);
        assert_eq!(sword.damage, IRON_SWORD_PVP_DAMAGE);
        assert_eq!(sword.knockback_speed, IRON_SWORD_KNOCKBACK_SPEED);
        assert_eq!(sword.reach_m, COMBAT_ATTACK_RANGE_M);
        assert_eq!(sword.cooldown_ticks, IRON_SWORD_COOLDOWN_TICKS);
        assert_eq!(sword.armor_pierce_pct, 0);
        assert_eq!(sword.model, ItemModel::Sword);

        let mace = weapon_profile(IRON_MACE_ID);
        assert_eq!(mace.damage, IRON_MACE_PVP_DAMAGE);
        assert_eq!(mace.knockback_speed, IRON_MACE_KNOCKBACK_SPEED);
        assert_eq!(mace.reach_m, COMBAT_ATTACK_RANGE_M);
        assert_eq!(mace.cooldown_ticks, IRON_MACE_COOLDOWN_TICKS);
        assert_eq!(mace.model, ItemModel::Mace);
        // The mace is the ONLY melee weapon that pierces armor (50%).
        assert_eq!(mace.armor_pierce_pct, IRON_MACE_ARMOR_PIERCE_PCT);
        assert_eq!(mace.armor_pierce_pct, 50);
        assert_eq!(club.armor_pierce_pct, 0);
        assert_eq!(spear.armor_pierce_pct, 0);
        assert_eq!(sword.armor_pierce_pct, 0);
    }

    #[test]
    fn mace_deals_the_most_damage_and_the_biggest_knockback() {
        use crate::game_balance::PICKAXE_KNOCKBACK_SPEED;
        use crate::items::{IRON_MACE_ID, IRON_SWORD_ID, STONE_SPEAR_ID, WOODEN_CLUB_ID};

        let club = weapon_profile(WOODEN_CLUB_ID);
        let spear = weapon_profile(STONE_SPEAR_ID);
        let sword = weapon_profile(IRON_SWORD_ID);
        let mace = weapon_profile(IRON_MACE_ID);

        // Damage climbs club < spear < sword < mace.
        assert!(club.damage < spear.damage);
        assert!(spear.damage < sword.damage);
        assert!(sword.damage < mace.damage);

        // The mace has the biggest knockback in the game, heavier even than the
        // pickaxe's 4.0 m/s shove.
        assert!(mace.knockback_speed > PICKAXE_KNOCKBACK_SPEED);
        for other in [club, spear, sword] {
            assert!(
                mace.knockback_speed > other.knockback_speed,
                "the mace out-shoves every other weapon"
            );
        }
    }

    #[test]
    fn weapon_cooldowns_order_club_fastest_then_sword_spear_mace() {
        use crate::items::{IRON_MACE_ID, IRON_SWORD_ID, STONE_SPEAR_ID, WOODEN_CLUB_ID};
        // Speed ordering is a hard design constraint: club < sword < spear < mace
        // in cooldown ticks (smaller = faster).
        let club = weapon_profile(WOODEN_CLUB_ID).cooldown_ticks;
        let sword = weapon_profile(IRON_SWORD_ID).cooldown_ticks;
        let spear = weapon_profile(STONE_SPEAR_ID).cooldown_ticks;
        let mace = weapon_profile(IRON_MACE_ID).cooldown_ticks;
        assert!(club < sword, "club is faster than the sword");
        assert!(sword < spear, "sword is faster than the spear");
        assert!(spear < mace, "spear is faster than the mace");
    }

    // ---- Armor pierce math ----

    #[test]
    fn pierce_reduces_effective_armor_before_mitigation() {
        // 0% pierce leaves armor untouched at every armor level.
        assert_eq!(effective_armor_after_pierce(0, 0), 0);
        assert_eq!(effective_armor_after_pierce(40, 0), 40);
        assert_eq!(effective_armor_after_pierce(100, 0), 100);

        // 50% pierce halves the effective armor (floored by integer division).
        assert_eq!(effective_armor_after_pierce(0, 50), 0);
        assert_eq!(effective_armor_after_pierce(40, 50), 20);
        assert_eq!(effective_armor_after_pierce(100, 50), 50);

        // 100% pierce zeroes armor entirely, so mitigation is fully bypassed.
        assert_eq!(effective_armor_after_pierce(0, 100), 0);
        assert_eq!(effective_armor_after_pierce(40, 100), 0);
        assert_eq!(effective_armor_after_pierce(100, 100), 0);
    }

    #[test]
    fn pierce_then_mitigation_lets_damage_through() {
        // The end-to-end shape the server uses: pierce first, then armor
        // reduction. 100 raw vs 40 armor is 60 through with no pierce; with 50%
        // pierce the effective armor drops to 20, letting 80 through.
        let raw = 100;
        let armor = 40;
        assert_eq!(
            damage_after_armor(raw, effective_armor_after_pierce(armor, 0)),
            60
        );
        assert_eq!(
            damage_after_armor(raw, effective_armor_after_pierce(armor, 50)),
            80
        );
        // Full pierce vs full armor still deals full damage.
        assert_eq!(
            damage_after_armor(raw, effective_armor_after_pierce(100, 100)),
            100
        );
    }

    #[test]
    fn pierce_clamps_out_of_range_values() {
        // Armor and pierce both clamp to 100 before the math so an over-stacked
        // buff can't wrap or over-shave.
        assert_eq!(effective_armor_after_pierce(250, 50), 50);
        assert_eq!(effective_armor_after_pierce(40, 250), 0);
    }
}
