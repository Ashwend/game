//! Centralized game-balance constants.
//!
//! Every tuneable that affects gameplay (combat ranges, gather windows,
//! interact distances, smelt timings, knockback shapes, respawn radii)
//! lives here. Each subsystem re-exports the constants it consumes, so
//! call sites stay short (`combat::ATTACK_RANGE_M`) while the actual
//! values live in one file an evals/balance-tuning pass can edit.
//!
//! Adding a new tuning knob:
//! 1. Declare it here with a `pub const NAME: T = value;` and a doc
//!    comment explaining what it controls and why this value.
//! 2. Re-export it from the owning subsystem module:
//!    `pub(crate) use crate::game_balance::NAME;`
//! 3. Reference it in code via the subsystem module path so the
//!    one-tunable-per-feature-area shape is preserved.
//!
//! Don't put balance values directly in subsystem files, even a
//! "throwaway" magic number is harder to find six months later. If a
//! value affects what the game feels like, it belongs in this file.

use crate::protocol::SERVER_TICK_RATE_HZ;

// =====================================================================
// Combat
// =====================================================================

/// Max feet-to-feet distance at which a melee swing connects. Mirrors
/// the client's pickup highlight range so "I aimed at them and they
/// were highlighted" ≈ "the server accepted the hit". A small
/// tolerance (0.5 m) over the client's value covers the
/// movement-prediction delta between the client's view at swing
/// time and the server's at receive time.
pub const COMBAT_ATTACK_RANGE_M: f32 = 3.5;

/// Margin, in metres, by which the client's *player*-attack targeting range sits
/// inside the swing's authoritative reach. The client only marks a remote player
/// as the swing target when they are within `AttackProfile::reach_m` minus this
/// margin, while the server validates the hit at the full `reach_m`. The gap
/// covers the movement-prediction delta between the client's view at swing time
/// and the server's at receive time, so a hit the client shows as landing is one
/// the server still accepts (never the "tooltip says reachable, server says no"
/// pop). Applies to player targeting only: a standard-reach weapon or tool (3.5 m)
/// targets players at 3.0 m and the spear (4.5 m reach) at 4.0 m. Non-player
/// targeting (nodes, deployables) keeps its own ranges.
pub const COMBAT_PLAYER_TARGET_REACH_MARGIN_M: f32 = 0.5;

/// Cosine of the attacker's view-cone half-angle.
pub const COMBAT_ATTACK_CONE_COS: f32 = 0.92;

/// Vertical offset from the attacker's feet to the eye, used as the
/// LOS ray origin server-side.
pub const COMBAT_ATTACKER_EYE_HEIGHT: f32 = 1.62;

/// Chest-height aim point relative to the target's feet, used as the
/// LOS ray's destination.
pub const COMBAT_TARGET_CHEST_HEIGHT: f32 = 0.95;

/// Player body hit box used by BOTH the client's swing targeting and the
/// server's hit validation (`crate::combat::player_body_ray_entry`), so "my
/// crosshair was on the avatar" and "the server accepted the hit" test the
/// exact same volume, no client/server drift. Half-extents roughly match the
/// controller capsule `(PLAYER_RADIUS, PLAYER_HEIGHT/2, PLAYER_RADIUS)`, a touch
/// larger so the hit volume is forgiving at strafe speed. `CENTRE_Y` is the
/// box centre above the feet (matches the avatar's visual centre).
pub const COMBAT_PLAYER_BODY_HALF_WIDTH: f32 = 0.40;
pub const COMBAT_PLAYER_BODY_HALF_HEIGHT: f32 = 0.95;
pub const COMBAT_PLAYER_BODY_CENTRE_Y: f32 = 0.95;

/// Hit box for a logged-out sleeping body: low and wide because the avatar is
/// laid flat on the ground, so looking at the sprawl anywhere lands the swing /
/// loot prompt. Same shared use as the standing box above.
pub const COMBAT_SLEEPING_BODY_HALF_WIDTH: f32 = 0.9;
pub const COMBAT_SLEEPING_BODY_HALF_HEIGHT: f32 = 0.4;
pub const COMBAT_SLEEPING_BODY_CENTRE_Y: f32 = 0.35;

/// Fraction of the horizontal knockback magnitude applied as a
/// vertical pop. Small upward component so the target slides away
/// instead of grinding into the floor on the first contact substep.
pub const COMBAT_KNOCKBACK_VERTICAL_FRACTION: f32 = 0.25;

/// Extra delay before the next swing can begin after a whiff (a swing
/// whose impact frame connected with nothing: no player, node, or
/// structure). A landed swing rolls straight into the next while LMB is
/// held; a miss pays this recovery gap first. The point is to punish
/// "hold left-click and pray" in PvP, a player who times their swings
/// to land hits keeps full cadence, while spraying at empty air costs
/// tempo. Kept deliberately small ("slight") so deliberate clicking
/// stays the better play without making a single miss feel like a stun.
pub const COMBAT_MISS_RECOVERY_SECONDS: f32 = 0.25;

/// When choosing a safe respawn spot, no other live player may be
/// closer than this distance. Prevents respawn camping.
pub const RESPAWN_MIN_DISTANCE_M: f32 = 12.0;

// =====================================================================
// Tools (durability + PvP damage)
// =====================================================================

/// Impacts a tier-1 (stone) tool survives before breaking. Only swings
/// that actually connect (a gather payout, a player hit, a structure
/// hit) consume durability; whiffs are free. At ~12 swings to drain a
/// full ore node this is roughly 16 nodes per stone tool, enough to
/// feel the wear without making the early game a re-crafting chore.
pub const STONE_TOOL_DURABILITY: u32 = 200;

/// Impacts a tier-2 (iron) tool survives. 3x the stone budget on top of
/// the 2x gather yield: the iron upgrade is felt both as bigger payouts
/// and as far fewer trips back to the workbench.
pub const IRON_TOOL_DURABILITY: u32 = 600;

/// Per-swing PvP damage for each tool. The hatchet stays the
/// fast/light option and the pickaxe the slow/heavy one within a tier,
/// while the iron tier hits ~1.5x harder than stone so weapon quality
/// tracks tool progression instead of being flat across tiers.
pub const STONE_HATCHET_PVP_DAMAGE: u32 = 8;
pub const IRON_HATCHET_PVP_DAMAGE: u32 = 12;
pub const STONE_PICKAXE_PVP_DAMAGE: u32 = 15;
pub const IRON_PICKAXE_PVP_DAMAGE: u32 = 22;

/// Magnitude of the PvP knockback impulse, in m/s, per tool kind. The
/// hatchet's light tap keeps melee chases tight; the pickaxe's heavy
/// shove is its compensation for the slower swing. Knockback stays a
/// kind-level trait (not per tier) so upgrading tools changes damage,
/// not the feel of getting hit.
pub const HATCHET_KNOCKBACK_SPEED: f32 = 1.8;
pub const PICKAXE_KNOCKBACK_SPEED: f32 = 4.0;

/// Max meteorite alloy a single pickaxe swing extracts from a meteorite node
/// (`ResourceNodeDefinition::per_swing_yield`). The node stores 8 total, so a
/// find is a deliberate 4-hit mining beat; without the cap the iron pickaxe's
/// 12 gather_amount would exhaust the whole rare node in one swing.
pub const METEORITE_PER_SWING_YIELD: u16 = 2;

// =====================================================================
// Weapons (dedicated melee: club, spear, sword, mace)
// =====================================================================
//
// The four melee weapons widen the tool spectrum rather than blur it: the
// hatchet (fast, light) and pickaxe (slow, heavy) are the reference points, and
// each weapon stakes out a distinct point between and beyond them. Combat
// resolves a weapon's `WeaponProfile` ahead of any gather tool (weapons gather
// nothing), so these are the numbers a swing actually reads.
//
// Cooldown ordering is a hard design constraint: club < sword < spear < mace,
// in server ticks (20 per second), so faster weapons have the smaller floor.
// The tool anchors are the stone hatchet / pickaxe at 6 ticks and the iron
// tools at 5; the club sits just slower than the stone hatchet and the rest
// step up from there. Reach is 3.5 m (the melee default) for everything except
// the spear, which trades speed for a 4.5 m poke. Durability reuses the tool
// tiers so a weapon's material tells you how long it lasts.

/// Wooden club: the starter weapon. A fast, cheap chop, slightly slower than the
/// stone hatchet (7 ticks vs 6) and hitting a touch harder (12 vs the hatchet's
/// 8), with a moderate shove. Stone-tier durability so it wears like the tools it
/// is crafted beside.
pub const WOODEN_CLUB_PVP_DAMAGE: u32 = 12;
pub const WOODEN_CLUB_KNOCKBACK_SPEED: f32 = 2.4;
pub const WOODEN_CLUB_COOLDOWN_TICKS: u64 = 7;

/// Stone spear: the space-control weapon. Slow (11 ticks) with the game's only
/// extended melee reach (4.5 m vs the 3.5 m standard), 16 damage, and a low
/// knockback (the point is to keep an enemy at the tip, not to fling them out of
/// range). Stone-tier durability.
pub const STONE_SPEAR_PVP_DAMAGE: u32 = 16;
pub const STONE_SPEAR_KNOCKBACK_SPEED: f32 = 1.2;
pub const STONE_SPEAR_COOLDOWN_TICKS: u64 = 11;
/// Extended melee reach for the spear, in metres, versus the 3.5 m default
/// (`COMBAT_ATTACK_RANGE_M`). This is the one weapon that reaches past standard
/// melee, its whole identity.
pub const STONE_SPEAR_REACH_M: f32 = 4.5;

/// Iron sword: the workhorse. A balanced 20 damage on a medium cooldown (9
/// ticks, between the club and the spear) with a moderate shove. Iron-tier
/// durability, so it outlasts the stone weapons the way iron tools outlast stone.
pub const IRON_SWORD_PVP_DAMAGE: u32 = 20;
pub const IRON_SWORD_KNOCKBACK_SPEED: f32 = 3.0;
pub const IRON_SWORD_COOLDOWN_TICKS: u64 = 9;

/// Iron mace: the anti-armor answer. The slowest weapon (14 ticks, slower even
/// than the hammer's 8) and the hardest single hit at 26, with the biggest
/// knockback in the game, heavier than the pickaxe's 4.0 m/s shove, so a landed
/// mace blow flings a target clear. Its 50% armor pierce is what makes it the
/// counter to a fully armored opponent: half their mitigation is ignored before
/// the hit lands. Iron-tier durability.
pub const IRON_MACE_PVP_DAMAGE: u32 = 26;
pub const IRON_MACE_KNOCKBACK_SPEED: f32 = 5.0;
pub const IRON_MACE_COOLDOWN_TICKS: u64 = 14;
/// Percent of the target's armor the mace ignores before mitigation. The
/// heaviest weapon punches through half of any set, its role in the
/// rock-paper-scissors: it punishes iron armor the way nothing else does.
pub const IRON_MACE_ARMOR_PIERCE_PCT: u8 = 50;

// =====================================================================
// Ranged (bow, crossbow, arrows, projectile simulation)
// =====================================================================
//
// Ranged weapons carry a `RangedProfile` (parallel to `WeaponProfile`) rather
// than firing a melee swing. The bow draws to build damage; the crossbow is a
// flat, slow, hard-hitting shot. Both spawn a server-simulated arrow that flies
// under gravity and resolves its own hit. Projectile damage is `DamageKind::
// Projectile`, so armor's projectile column (and the mace's pierce path) apply
// through the shared post-hit tail. Draw and cooldown are in server ticks
// (20 per second). Speeds are in metres per second.

/// Wooden bow: minimum draw damage (an instant, no-hold release). Damage lerps
/// from this floor up to the full-draw ceiling over `WOODEN_BOW_DRAW_TICKS`.
pub const WOODEN_BOW_DAMAGE_MIN: u32 = 15;
/// Wooden bow: full-draw damage ceiling, reached after holding the draw for the
/// full window. A committed, aimed shot rewards the wait.
pub const WOODEN_BOW_DAMAGE_MAX: u32 = 40;
/// Wooden bow: arrow launch speed, in metres per second. Still slower and
/// loopier than the crossbow (the bow stays the arcing option to the
/// crossbow's snap shot), but raised from the original 35: at that speed
/// arrows nosed into the ground a few metres out and shots felt powerless
/// (owner report), so the arc now carries a proper fighting distance.
pub const WOODEN_BOW_PROJECTILE_SPEED_MPS: f32 = 50.0;
/// Wooden bow: ticks from draw start to full draw (1.5 s at 20 Hz). Damage scales
/// linearly across this window; releasing early deals proportionally less.
pub const WOODEN_BOW_DRAW_TICKS: u64 = (1.5 * SERVER_TICK_RATE_HZ) as u64;
/// Wooden bow: server anti-spam floor between shots, in ticks. Small; the draw
/// time is the real pacing lever, not the post-fire cooldown.
pub const WOODEN_BOW_COOLDOWN_TICKS: u64 = 5;
/// Bow: minimum draw fraction for a release to fire at all. Below this the
/// release is a cancel, not a shot: tapping the button can never loose an
/// arrow (owner requirement), the archer must commit at least this much of the
/// draw. Enforced on the server off its own observed draw ticks; the client
/// mirrors it so a too-short release lowers the bow instead of firing.
/// Instant-fire weapons (crossbow, `draw_ticks_to_full == 0`) are exempt.
pub const BOW_MIN_DRAW_FRACTION_TO_FIRE: f32 = 0.25;
/// Bow: launch speed at the minimum firing draw, as a fraction of the profile's
/// full `projectile_speed_mps`. Speed lerps from this floor at zero draw up to
/// full speed at full draw, so a barely-held shot lobs out weak and short while
/// a committed draw sends the arrow at full pace (owner requirement: power
/// follows the hold). Damage already scales the same way via the profile's
/// damage band.
pub const BOW_MIN_RELEASE_SPEED_FRACTION: f32 = 0.45;
/// Wooden bow: knockback impulse magnitude on a hit, in m/s. A committed,
/// full-draw arrow shoves the target back noticeably (roughly a sword's worth of
/// push), so a landed shot reads as a real hit rather than a pinprick.
pub const WOODEN_BOW_KNOCKBACK_SPEED: f32 = 2.5;

/// Crossbow: flat per-shot damage. No draw scaling (`CROSSBOW_DRAW_TICKS = 0`);
/// every bolt hits for this, which is why the reload is long.
pub const CROSSBOW_DAMAGE: u32 = 55;
/// Crossbow: bolt launch speed, in metres per second. Faster and flatter than the
/// bow, the ambush weapon: less lead, less drop, but a punishing reload. Bumped a
/// touch over the old 55 so the bolt reads as a snap shot with almost no lead.
pub const CROSSBOW_PROJECTILE_SPEED_MPS: f32 = 62.0;
/// Crossbow: draw window in ticks. Zero: the crossbow is pre-loaded, so a shot is
/// always at full damage the instant the reload cooldown has elapsed.
pub const CROSSBOW_DRAW_TICKS: u64 = 0;
/// Crossbow: reload cooldown between shots, in ticks (3.5 s at 20 Hz). This is
/// the crossbow's whole cost: 55 flat damage, but a long, audible ratchet before
/// the next bolt.
pub const CROSSBOW_COOLDOWN_TICKS: u64 = (3.5 * SERVER_TICK_RATE_HZ) as u64;
/// Crossbow: knockback impulse magnitude on a hit, in m/s. The heavy hitter of
/// the two: a 55-damage bolt lands with a real shove (well past the sword and
/// near the mace), so eating one clearly rocks the target back.
pub const CROSSBOW_KNOCKBACK_SPEED: f32 = 4.0;

/// Downward acceleration applied to a live projectile each second, in m/s^2. A
/// gamey arc rather than real-world 9.81: strong enough that arrows visibly drop
/// and lead matters, gentle enough that the bow stays usable at mid range.
pub const PROJECTILE_GRAVITY: f32 = -12.0;

/// Hard cap on a projectile's flight time before it despawns, in seconds. Bounds
/// the per-tick projectile set so a shot into open sky can never linger forever.
pub const PROJECTILE_MAX_FLIGHT_SECONDS: f32 = 8.0;

/// Ticks after spawn during which a projectile cannot hit its own shooter. Keeps
/// the arrow from resolving against the shooter's own body box on the first frame
/// (the projectile spawns at the shooter's eye, inside their collider column).
pub const PROJECTILE_SELF_HIT_GRACE_TICKS: u64 = 4;

/// How long a stuck arrow lingers after a projectile comes to rest against the
/// world before it despawns, in seconds. Every world rest sticks and is
/// E-recoverable for this window; an uncollected arrow is lost when it expires,
/// which (with hits consuming the arrow outright) is the whole ammo economy.
pub const PROJECTILE_STUCK_TTL_SECONDS: f32 = 30.0;

/// Speed, in m/s, of the epsilon rest velocity a stuck arrow keeps. Far below
/// any stuck-detection threshold, so it never reads as motion; its only job is
/// to carry the final flight DIRECTION on the wire so every client (including
/// one that first sees the arrow already at rest) orients the stuck shaft along
/// the shot that planted it instead of pointing it straight up.
pub const PROJECTILE_REST_DIR_EPSILON: f32 = 0.01;

/// Effectiveness of a projectile hit against a placed deployable, in percent of
/// the projectile's damage. Weapons (including ranged) are not raid tools:
/// arrows chip sticks-tier structures only and do a token amount against anything
/// sturdier, so a base is never raidable with a bow. Mirrors the sticks arm of
/// `tool_effectiveness_pct` in spirit, expressed as one ranged-specific rule.
pub const PROJECTILE_DEPLOYABLE_EFFECTIVENESS_PCT: u32 = 100;

/// Run-speed multiplier applied to a player while their bow draw is held. The
/// draw slows movement to ~60% so drawing is a commitment, not a free kite. Set
/// on the existing `run_speed_multiplier` lever on draw start and restored to
/// `1.0` on fire, cancel, item swap, or death.
pub const BOW_DRAW_MOVE_MULTIPLIER: f32 = 0.6;

/// Run-speed multiplier applied to a player while the crossbow is reloading (its
/// long post-fire cooldown window). Cranking a windlass while sprinting makes no
/// sense: the reload impairs movement to ~70% (a lighter penalty than the bow
/// draw, since the whole reload is already a hard commitment) so ambushing with a
/// crossbow costs mobility on the recovery. Set on the `run_speed_multiplier`
/// lever the instant the shot fires and restored to `1.0` when the reload window
/// (`next_ranged_tick`) elapses, on item swap, or on death.
pub const CROSSBOW_RELOAD_MOVE_MULTIPLIER: f32 = 0.7;

// =====================================================================
// Armor (worn-equipment mitigation)
// =====================================================================

/// Hard ceiling on total damage reduction from worn armor, per damage kind, in
/// percent. Protection sums across the four worn pieces and is then clamped to
/// this cap, so no full set (or future stacked set) can push mitigation past
/// it; a player always takes at least `100 - cap` percent of every hit. 60% is
/// the spec's ceiling: enough that a full iron set is a real advantage, low
/// enough that armor never trivializes combat.
pub const ARMOR_TOTAL_CAP_PCT: u8 = 60;

/// Per-piece protection for the padded (cloth) set, in percent, per damage
/// kind. The set totals (melee 12 / projectile 10 / blast 4) are split across
/// the four slots by the spec's weighting (chest 40%, head 25%, legs 25%, feet
/// 10%) and integer-rounded so each column sums exactly to its set total:
///
/// - melee:      head 3 + chest 5 + legs 3 + feet 1 = 12
/// - projectile: head 3 + chest 4 + legs 2 + feet 1 = 10
/// - blast:      head 1 + chest 2 + legs 1 + feet 0 = 4
///
/// The rounding lands the extra melee/blast weight on the chest (the biggest
/// slot) and the odd projectile percent on the head, so the "wear the chest
/// piece first" instinct is rewarded.
pub const PADDED_HEAD_MELEE_PCT: u8 = 3;
pub const PADDED_HEAD_PROJECTILE_PCT: u8 = 3;
pub const PADDED_HEAD_BLAST_PCT: u8 = 1;
pub const PADDED_CHEST_MELEE_PCT: u8 = 5;
pub const PADDED_CHEST_PROJECTILE_PCT: u8 = 4;
pub const PADDED_CHEST_BLAST_PCT: u8 = 2;
pub const PADDED_LEGS_MELEE_PCT: u8 = 3;
pub const PADDED_LEGS_PROJECTILE_PCT: u8 = 2;
pub const PADDED_LEGS_BLAST_PCT: u8 = 1;
pub const PADDED_FEET_MELEE_PCT: u8 = 1;
pub const PADDED_FEET_PROJECTILE_PCT: u8 = 1;
pub const PADDED_FEET_BLAST_PCT: u8 = 0;

/// Impacts a padded (cloth) armor piece survives before it stops protecting.
/// Each hit that a piece's protection contributes to wears it by 1; a piece at
/// 0 stays worn but adds nothing until repaired. A modest budget for the
/// starter set: enough to matter across a fight, cheap enough to re-craft.
pub const PADDED_ARMOR_DURABILITY: u32 = 100;

/// Per-piece protection for the lamellar (wood slats over cloth) set, in
/// percent, per damage kind. The set totals (melee 24 / projectile 20 / blast
/// 10) are split across the four slots by the spec's weighting (chest 40%, head
/// 25%, legs 25%, feet 10%) and integer-rounded so each column sums exactly to
/// its set total:
///
/// - melee:      head 6 + chest 10 + legs 6 + feet 2 = 24
/// - projectile: head 5 + chest 8  + legs 5 + feet 2 = 20
/// - blast:      head 2 + chest 5  + legs 2 + feet 1 = 10
///
/// As with the padded set, the rounding lands the extra weight on the chest (the
/// biggest slot), so wearing the vest first is always the strongest single
/// choice. Every column is exercised by `full_lamellar_set_sums_to_the_spec_totals`.
pub const LAMELLAR_HEAD_MELEE_PCT: u8 = 6;
pub const LAMELLAR_HEAD_PROJECTILE_PCT: u8 = 5;
pub const LAMELLAR_HEAD_BLAST_PCT: u8 = 2;
pub const LAMELLAR_CHEST_MELEE_PCT: u8 = 10;
pub const LAMELLAR_CHEST_PROJECTILE_PCT: u8 = 8;
pub const LAMELLAR_CHEST_BLAST_PCT: u8 = 5;
pub const LAMELLAR_LEGS_MELEE_PCT: u8 = 6;
pub const LAMELLAR_LEGS_PROJECTILE_PCT: u8 = 5;
pub const LAMELLAR_LEGS_BLAST_PCT: u8 = 2;
pub const LAMELLAR_FEET_MELEE_PCT: u8 = 2;
pub const LAMELLAR_FEET_PROJECTILE_PCT: u8 = 2;
pub const LAMELLAR_FEET_BLAST_PCT: u8 = 1;

/// Impacts a lamellar armor piece survives before it stops protecting. Sits
/// between the padded starter set (100) and the iron plate set (300): the wood
/// slats hold up longer than bare cloth but nowhere near forged plate.
pub const LAMELLAR_ARMOR_DURABILITY: u32 = 200;

/// Per-piece protection for the iron (plate over padding) set, in percent, per
/// damage kind. The set totals (melee 40 / projectile 36 / blast 20) are split
/// across the four slots by the spec's weighting (chest 40%, head 25%, legs
/// 25%, feet 10%) and integer-rounded so each column sums exactly to its set
/// total:
///
/// - melee:      head 10 + chest 16 + legs 10 + feet 4 = 40
/// - projectile: head 9  + chest 14 + legs 9  + feet 4 = 36
/// - blast:      head 5  + chest 8  + legs 5  + feet 2 = 20
///
/// A full iron set sums to 40 melee, under the 60% cap, so even the top set
/// leaves a real chunk of every hit landing. Every column is exercised by
/// `full_iron_set_sums_to_the_spec_totals`.
pub const IRON_HEAD_MELEE_PCT: u8 = 10;
pub const IRON_HEAD_PROJECTILE_PCT: u8 = 9;
pub const IRON_HEAD_BLAST_PCT: u8 = 5;
pub const IRON_CHEST_MELEE_PCT: u8 = 16;
pub const IRON_CHEST_PROJECTILE_PCT: u8 = 14;
pub const IRON_CHEST_BLAST_PCT: u8 = 8;
pub const IRON_LEGS_MELEE_PCT: u8 = 10;
pub const IRON_LEGS_PROJECTILE_PCT: u8 = 9;
pub const IRON_LEGS_BLAST_PCT: u8 = 5;
pub const IRON_FEET_MELEE_PCT: u8 = 4;
pub const IRON_FEET_PROJECTILE_PCT: u8 = 4;
pub const IRON_FEET_BLAST_PCT: u8 = 2;

/// Impacts an iron (plate) armor piece survives before it stops protecting. The
/// highest durability of the three sets: forged plate outlasts wood slats and
/// cloth alike, matching its top-tier cost.
pub const IRON_ARMOR_DURABILITY: u32 = 300;

/// Maximum distance at which the cosmetic impact messages
/// (`ResourceImpact`, `PlayerImpact`) are delivered. Spatial audio
/// attenuates to silence and chip bursts are sub-pixel well inside this
/// radius, so clients farther away can neither hear nor see the effect;
/// without the gate every swing on the server was broadcast to every
/// connected client (N x M messages per second while gathering).
pub const IMPACT_MESSAGE_RANGE_M: f32 = 80.0;

/// Delivery range for the dropped-item merge cue (`ItemMerged`). The cue
/// is a quiet UI blip acknowledging that two nearby drops fused; players
/// elsewhere on the map have no use for it.
pub const ITEM_MERGE_CUE_RANGE_M: f32 = 25.0;

// =====================================================================
// Deployables (workbenches, furnaces, walls, …)
// =====================================================================

/// Maximum distance at which a player can damage a placed structure.
/// Kept close to the player melee range (`COMBAT_ATTACK_RANGE_M`) so you
/// have to stand next to a workbench/furnace to hit it rather than chipping
/// it from across the room; a little extra over melee accounts for the
/// structure's body size (the check is feet-to-centre). Kept equal to
/// `FURNACE_INTERACT_RANGE_M` so the swing/open flow stays consistent: if E
/// reaches it, your tool reaches it too. The client targeting in
/// `app::systems::items::pickup::targets` and the nameplate overlay both
/// derive their ranges from this constant, so this is the single tuning knob.
pub const DEPLOYABLE_DAMAGE_RANGE_M: f32 = 3.0;

/// Per-tool damage scalar. The tool's `gather_amount` already scales
/// with tier (stone tools = 6, future iron tools = higher), so re-using
/// it as the base means deployable damage tracks tool tier without a
/// separate balance table. The multiplier puts stone-tool
/// time-to-destroy in the survival-game-sweet-spot (~15 swings for a
/// workbench).
pub const DEPLOYABLE_DAMAGE_PER_GATHER_POINT: u32 = 5;

/// Maximum distance from the placing player's feet to the requested
/// placement position. Keeps placements within arm's reach + a forgiving
/// margin for foot-of-camera vs centre-of-feet projection.
pub const DEPLOYABLE_PLACEMENT_REACH_M: f32 = 5.0;

// =====================================================================
// Base building (building blocks, hammer, doors, sleeping bags)
// =====================================================================

/// Wall-piece HP per tier (foundations carry 1.5x, see
/// `building::building_max_health`). The numbers are chosen against the
/// `tool_effectiveness_pct` building arms so that:
/// - Sticks fall in a few swings of any proper tool (a stone hatchet at
///   300% does 90/hit, three hits and the wall is kindling).
/// - Hewn wood is raidable with tools but slow: an iron hatchet at 15% does
///   9/hit, so a wall costs ~400 swings (~5 minutes of continuous
///   swinging) and most of the tool's 600 durability. Possible, loud,
///   and expensive, exactly the "soft side" feel.
/// - Stone takes zero damage from every tool (0% arms), so tool-raiding
///   a stone base is impossible by construction. The HP still matters
///   for future siege equipment.
pub const BUILDING_STICKS_WALL_HP: u32 = 250;
pub const BUILDING_HEWN_WOOD_WALL_HP: u32 = 3_600;
pub const BUILDING_STONE_WALL_HP: u32 = 6_000;

/// Placement costs (always at the Sticks tier, paid in raw wood;
/// upgrades pay the tier costs below). A hatchet swing on a tree yields
/// 6 wood, so a starter 1x1 (foundation + 3 walls + doorway) is a short
/// gathering loop, not an afternoon.
pub const BUILDING_STICKS_COST_FOUNDATION: u16 = 30;
pub const BUILDING_STICKS_COST_WALL: u16 = 25;

/// Upgrade costs to the hewn-wood tier, paid in workbench-refined hewn
/// logs (10 raw wood each), so a wall upgrade is ~100 wood-equivalent
/// plus the bench time, matching the old raw-wood cost while making the
/// tier gate the workbench.
pub const BUILDING_HEWN_WOOD_COST_FOUNDATION: u16 = 12;
pub const BUILDING_HEWN_WOOD_COST_WALL: u16 = 10;

/// Upgrade costs to the stone tier.
pub const BUILDING_STONE_COST_FOUNDATION: u16 = 150;
pub const BUILDING_STONE_COST_WALL: u16 = 125;

/// Materials consumed by one hammer repair hit, in the piece's own tier
/// material. Each hit restores `BUILDING_REPAIR_FRACTION_PCT` of max HP.
pub const BUILDING_REPAIR_COST_STICKS: u16 = 5;
pub const BUILDING_REPAIR_COST_HEWN_WOOD: u16 = 2;
pub const BUILDING_REPAIR_COST_STONE: u16 = 20;

/// Percentage of max HP restored per hammer repair hit.
pub const BUILDING_REPAIR_FRACTION_PCT: u32 = 25;

/// How long after placement (or upgrade) the owner's hammer can still
/// demolish a piece, in ticks. 15 minutes: long enough to fix layout
/// mistakes, short enough that a compromised base can't be deleted out
/// from under a raid by its panicking owner.
pub const BUILDING_DEMOLISH_WINDOW_TICKS: u64 = (15.0 * 60.0 * SERVER_TICK_RATE_HZ) as u64;

/// Structural stability: percentage of support retained per vertical hop
/// (wall on a platform or on another wall, ceiling on a wall, stairs on
/// a platform). 90% halves a tower's stability roughly every 6 storeys,
/// pairing with the placement minimum below to cap practical height.
pub const STABILITY_RETENTION_VERTICAL_PCT: u32 = 90;

/// Stability retained by a ceiling hanging off an adjacent ceiling (a
/// cantilevered ledge). 35% per tile cuts a first-storey overhang off at
/// two tiles past the carrying wall (81 -> 28 -> 9, under the placement
/// minimum), so real roofs need walls under them, not chains of ledges.
pub const STABILITY_RETENTION_CEILING_NEIGHBOR_PCT: u32 = 35;

/// Minimum stability a new piece must compute to be placeable. Pieces
/// whose support drops to exactly zero (their ground path is gone) are
/// destroyed on the next structural update.
pub const BUILDING_MIN_PLACEMENT_STABILITY_PCT: u32 = 10;

/// How far above the ground a free-placed foundation may sit. Raising is
/// aim-driven during placement; the foundation mesh carries a skirt deep
/// enough to reach the ground at this raise, so elevated platforms never
/// float visually. Snapped extensions inherit their neighbour's height
/// instead of consulting this band.
pub const FOUNDATION_RAISE_MAX_M: f32 = 1.5;

/// How far below the ground a foundation base may sink. A quarter of the
/// platform height: enough to hug small terrain wobble without letting
/// the slab disappear into the ground.
pub const FOUNDATION_SINK_MAX_M: f32 = 0.25;

/// Hewn log door HP. WoodBuilding material, so it's the designated soft
/// spot of a stone base: an iron hatchet chews through in ~2.5 minutes.
pub const DOOR_MAX_HP: u32 = 1_500;

/// Iron door HP. Double the wood door, but the HP barely matters for
/// raiding: the iron door uses the `MetalBuilding` material, which every
/// tool does 0 damage to (see `tool_effectiveness_pct`), so it's
/// tool-proof by construction and only future explosives will breach it.
/// The HP is what those explosives will chew through.
pub const IRON_DOOR_MAX_HP: u32 = 3_000;

/// Sleeping bag HP. Cloth tears fast; bags are respawn anchors, not cover.
pub const SLEEPING_BAG_MAX_HP: u32 = 100;

/// Storage box HP (plain Wood material, so any proper tool opens one up
/// eventually). Boxes are loot pinatas by design: keeping valuables safe
/// is what walls and doors are for.
pub const STORAGE_BOX_SMALL_HP: u32 = 400;
pub const STORAGE_BOX_LARGE_HP: u32 = 700;

/// Tool Cupboard HP. WoodBuilding-band durability so destroying it (which
/// lifts the base's building privilege) is a real raid objective: an iron
/// hatchet chews through over a couple of minutes, but it shrugs off
/// casual griefing. Not stone-immune by design, the claim should fall to
/// a committed raid.
pub const TOOL_CUPBOARD_MAX_HP: u32 = 1_000;

/// Building-privilege margin, in 3 m grid cells, that a Tool Cupboard's
/// claim projects outward from its base's connected footprint. Non-
/// authorized players can't place construction within this ring of a
/// claimed base, so a griefer can't wall someone in from just outside
/// their walls (the boundary-dead-zone problem a bare radius has). At 5
/// cells (~15 m) the buffer keeps raid bases a real distance off, not
/// butted against the wall.
pub const BUILDING_PRIVILEGE_MARGIN_CELLS: i32 = 5;

/// Slot counts for the two storage box sizes. Small is a starter stash
/// (most of an inventory row); large holds a serious stockpile but still
/// less than the player's own pack, so banking always costs trips.
pub const STORAGE_BOX_SMALL_SLOT_COUNT: usize = 8;
pub const STORAGE_BOX_LARGE_SLOT_COUNT: usize = 18;

/// Max range for opening (and continuing to use) a placed storage box.
/// Matches `FURNACE_INTERACT_RANGE_M` so every "press E on a structure"
/// interaction feels identical.
pub const STORAGE_BOX_INTERACT_RANGE_M: f32 = 3.0;

/// Allowed door code lengths (digits). Four digits is the genre classic;
/// six leaves room for the paranoid.
pub const DOOR_CODE_MIN_LEN: usize = 4;
pub const DOOR_CODE_MAX_LEN: usize = 6;

/// Longest sleeping bag name the server accepts after trimming.
pub const SLEEPING_BAG_NAME_MAX_LEN: usize = 24;

/// Longest world-map marker label the server accepts after trimming.
pub const WORLD_MAP_MARKER_NAME_MAX_LEN: usize = 24;

/// Cap on how many markers a single player can keep on their map. Generous
/// enough to flag every point of interest on a large world, low enough that a
/// client can't bloat the save with an unbounded list.
pub const WORLD_MAP_MARKER_MAX_PER_PLAYER: usize = 100;

/// Hammer durability: same budget as an iron tool, repairs and upgrades
/// are frequent but cheap taps.
pub const HAMMER_DURABILITY: u32 = 600;

// =====================================================================
// Torch
// =====================================================================

/// Torch HP. Wood and resin, light and flammable, easily knocked out.
pub const TORCH_MAX_HP: u32 = 60;

/// How long a placed torch burns before going dark, in ticks (~8 hours of
/// real time). When it burns out the torch stays placed but unlit; it can
/// still be destroyed like any deployable.
pub const TORCH_BURN_TICKS: u32 = (8.0 * 3600.0 * SERVER_TICK_RATE_HZ) as u32;

// =====================================================================
// Furnace
// =====================================================================

/// How long one smelt operation takes, in ticks. 6 seconds at the
/// server tick rate so it feels like a real wait without being
/// tedious for solo testing.
pub const FURNACE_SMELT_TICKS_PER_OUTPUT: u32 = (6.0 * SERVER_TICK_RATE_HZ) as u32;
/// Burn duration in ticks for one wood unit (4 s), short burn, lots
/// of shovelling.
pub const FURNACE_WOOD_BURN_TICKS: u32 = (4.0 * SERVER_TICK_RATE_HZ) as u32;
/// Burn duration in ticks for one coal unit (16 s). The upgrade path
/// from wood.
pub const FURNACE_COAL_BURN_TICKS: u32 = (16.0 * SERVER_TICK_RATE_HZ) as u32;

/// Maximum interaction range, in metres, for `E`-to-open. Kept equal to
/// `DEPLOYABLE_DAMAGE_RANGE_M` so opening and hitting a furnace use the same
/// reach: you stand next to it to use it. This is below
/// `DEPLOYABLE_PLACEMENT_REACH_M`, so a furnace placed at max reach needs a
/// step forward to open, an intentional trade for not interacting from afar.
pub const FURNACE_INTERACT_RANGE_M: f32 = 3.0;

// =====================================================================
// Workbench
// =====================================================================

/// Maximum interaction range, in metres, for opening (and continuing to use)
/// a placed workbench's upgrade UI. Kept equal to `FURNACE_INTERACT_RANGE_M`
/// so every "press E on a structure" interaction feels identical: you stand
/// next to the bench to work at it.
pub const WORKBENCH_INTERACT_RANGE_M: f32 = 3.0;

// =====================================================================
// Loot bags
// =====================================================================

/// Maximum interaction range, in metres, for opening a loot bag. The
/// drop-on-death bag is meant to be approached, not looted from
/// across the room.
pub const LOOT_BAG_INTERACT_RANGE_M: f32 = 4.5;

// =====================================================================
// Pickup (E-pickup of dropped items + crude resource nodes)
// =====================================================================

/// Extra reach, in metres, the *server* grants when accepting a pickup
/// beyond the client's strict view-ray range. Movement is
/// client-authoritative, so by the time a pickup command lands the player
/// has often moved or flicked their view; re-running the client's strict
/// view-cone test server-side would then reject pickups the player
/// legitimately made, which the client (having predicted the pickup) has to
/// visibly roll back, the "client says yes, server says no" pop. The server
/// instead does a generous distance-only check (the client already chose
/// *which* item via the view ray and only sends a command for a target it
/// accepted), trading a little reach for far fewer false rejects. Picking up
/// a nearby item you already targeted is low-stakes, so erring lenient here
/// costs nothing and feels much smoother while sprinting around.
pub const PICKUP_SERVER_REACH_SLACK_M: f32 = 1.5;

// =====================================================================
// Exploration worldgen
// =====================================================================

/// Minimum distance from world centre, as a fraction of the playable radius,
/// before a meteorite node may spawn. Meteorite is the rare, iron-gated,
/// night-glowing mineral that gates the workbench tier-2 upgrade and later explosives, so it lives in the committed outer reaches of the map: a chunk whose
/// centre sits inside this ring (roughly the inner 40% of the map radius) never
/// seeds meteorite, no matter how rich its ore channel is. The check is against
/// the world origin `(0, 0)` (the world is centred there) using the same
/// `PlayableBounds` half-extent both generation and the regrow ceiling read, so
/// the two stay in lockstep. Pushing outward is the price of the rare mineral.
pub const METEORITE_MIN_CENTER_DISTANCE_FRACTION: f32 = 0.4;

/// Ore-channel floor an eligible far chunk must clear before it seeds its lone
/// meteorite node. Set high on purpose: combined with the base capacity of 1
/// and the distance ring, this makes meteorite roughly an order of magnitude
/// rarer than iron, so most eligible chunks hold none and finding one is a real
/// discovery. Mirrors the `FOREST_IRON_ORE_CHANNEL` lucky-strike idiom, but
/// stricter.
pub const METEORITE_ORE_CHANNEL_FLOOR: f32 = 0.72;

// =====================================================================
// Ruins (POI worldgen) and loot caches
// =====================================================================

/// Minimum spacing, in metres, between two ruin sites. Ruins are landmarks a
/// player travels between, so they sit far apart: the scatter rejects a
/// candidate site that lands within this distance of an already-accepted one.
/// At ~180 m no two ruins share a chunk, and a medium world (~1984 m across)
/// fits only a handful, keeping each a genuine destination.
pub const RUIN_MIN_SPACING_M: f32 = 180.0;

/// Radius around world centre, as a fraction of the playable radius, inside
/// which no ruin may spawn. The spawn point sits at the origin, so this keeps
/// the ruins (and their PvP-contested caches) out of the fresh-spawn safe area
/// and gives exploration a reason: roughly the inner 15% of the map is
/// ruin-free. Measured against the same `PlayableBounds` half-extent the
/// meteorite ring uses, so the two gates read consistently.
pub const RUIN_SPAWN_EXCLUSION_RADIUS_FRACTION: f32 = 0.15;

/// Margin, in metres, kept between a ruin site and the edge of `PlayableBounds`
/// so a ruin's footprint (and its perimeter blocks) never clip the world wall.
/// Sized above the largest prefab half-extent.
pub const RUIN_BOUNDS_MARGIN_M: f32 = 12.0;

/// Extra ring, in metres, past a ruin's footprint circle inside which players
/// cannot place anything (deployables, torches, building pieces). Ruins are
/// shared, contested loot spots: without the gate a player walls in the
/// salvage chests or drops a sleeping bag next to one and camps the restock.
/// Explosive charges are exempt (raid tools stay usable anywhere; the chests
/// are indestructible regardless). Small on purpose so bases can still be
/// built within sight of a ruin.
pub const RUIN_PLACEMENT_EXCLUSION_MARGIN_M: f32 = 3.0;

/// Number of candidate sites the scatter samples per world before it stops.
/// Rejection sampling against the spacing + exclusion + bounds rules keeps only
/// the winners, so this caps how densely a map fills: too high and every map
/// saturates to its packing limit (a ruin every 180 m everywhere, which reads
/// as clutter, not landmarks). Tuned so a medium world (~1984 m) lands roughly
/// a dozen genuine destinations and a large world proportionally more.
/// Deterministic: the same seed always draws the same candidates in order.
pub const RUIN_SCATTER_CANDIDATES: u32 = 40;

/// HP stored on the ruin-cache deployable. Nominal only: the damage path
/// rejects the cache before any HP is subtracted, so this value is never
/// actually depleted. Present because every deployable carries a max_health.
pub const RUIN_CACHE_MAX_HP: u32 = 1_000;

/// Slot count of a ruin cache's loot grid. Small on purpose: a cache is a
/// quick grab, not a stockpile, and the refill rolls a handful of stacks.
pub const RUIN_CACHE_SLOT_COUNT: usize = 6;

/// Max range for opening a ruin cache. Matches the storage-box / furnace
/// interact range so every "press E on a structure" feels identical.
pub const RUIN_CACHE_INTERACT_RANGE_M: f32 = 3.0;

/// Real-world minutes between a cache being emptied and its refill firing.
/// Long enough that a cache is a periodic reason to return to a ruin rather
/// than a farm, short enough that a cleared ruin is worth revisiting in a
/// session. Expressed in ticks below.
pub const RUIN_CACHE_REFILL_MINUTES: f32 = 25.0;

/// Refill delay in server ticks. Derived from `RUIN_CACHE_REFILL_MINUTES` and
/// `SERVER_TICK_RATE_HZ` so the minutes constant is the single knob (mirrors
/// the `TORCH_BURN_TICKS` idiom).
pub const RUIN_CACHE_REFILL_TICKS: u64 =
    (RUIN_CACHE_REFILL_MINUTES * 60.0 * SERVER_TICK_RATE_HZ) as u64;

/// Ruin-cache loot table. `salvaged_fittings` is guaranteed on every roll (the
/// cache is its exclusive source), between these bounds inclusive.
pub const RUIN_CACHE_FITTINGS_MIN: u32 = 2;
pub const RUIN_CACHE_FITTINGS_MAX: u32 = 4;

/// Number of weighted secondary rolls per refill (gunpowder / iron_bar / cloth).
/// Each roll picks one entry by weight and adds its stack, so a refill yields
/// the guaranteed fittings plus this many common-material stacks.
pub const RUIN_CACHE_SECONDARY_ROLLS: u32 = 3;

/// Relative weights for the secondary loot roll. Higher is more likely; the
/// three must not all be zero. Gunpowder leads (it is the raid feedstock the
/// cache is meant to seed), iron_bar and cloth trail.
pub const RUIN_CACHE_WEIGHT_GUNPOWDER: u32 = 5;
pub const RUIN_CACHE_WEIGHT_IRON_BAR: u32 = 3;
pub const RUIN_CACHE_WEIGHT_CLOTH: u32 = 2;

/// Stack size a single secondary roll grants for each material.
pub const RUIN_CACHE_GUNPOWDER_PER_ROLL: u16 = 4;
pub const RUIN_CACHE_IRON_BAR_PER_ROLL: u16 = 2;
pub const RUIN_CACHE_CLOTH_PER_ROLL: u16 = 3;

/// Chance, in percent, that a restock also drops a single chunk of meteorite
/// alloy (a fragment of the strike that burnt the house down). The rare bonus
/// that makes a chest worth opening even when a player is not short on
/// fittings.
pub const RUIN_CACHE_METEORITE_CHANCE_PCT: u32 = 8;

// =====================================================================
// Meteor shower event
// =====================================================================

/// Minimum and maximum in-game days between scheduled meteor shower events.
/// The scheduler rolls the next event uniformly in this window (converted to
/// real server ticks via `REAL_SECONDS_PER_DAY` at cycle multiplier 1). Two to
/// four in-game days keeps the event a periodic, anticipated flashpoint without
/// letting it dominate a session. NOTE: these are *in-game* days measured
/// against the fixed real-time tick clock, so the admin `/time-speed` cheat
/// (which accelerates only the day/night cycle, not the wall clock) does NOT
/// pull meteors closer together; the schedule is real-time.
pub const METEOR_SHOWER_INTERVAL_DAYS_MIN: f32 = 2.0;
pub const METEOR_SHOWER_INTERVAL_DAYS_MAX: f32 = 4.0;

/// Real seconds between the announce (fireball appears, countdown starts) and
/// impact. Ten real minutes: long enough that a player anywhere on the map can
/// see the streak, read the countdown, and either rush the impact site to
/// contest the crater cluster or evacuate the danger zone. Tunable, an Eco-style
/// longer approach is just a larger value here.
pub const METEOR_SHOWER_WARNING_SECONDS: f32 = 600.0;

/// Clearance, in metres, kept between a chosen impact site and ANY player
/// structure (building piece, deployed entity, or Tool Cupboard claim
/// footprint). Building safety is guaranteed by SITING, never by a damage
/// exemption, so this MUST exceed `METEOR_SHOWER_IMPACT_RADIUS_M` (asserted by
/// `clearance_exceeds_impact_radius`): a base can never be inside the blast.
/// Sized well past the impact radius so even a base's outer wall stays clear of
/// the crater's edge.
pub const METEOR_SHOWER_BUILDING_CLEARANCE_M: f32 = 60.0;

/// Radius, in metres, of the meteor's lethal impact. Players inside take Blast
/// damage with linear falloff from ground zero; resource nodes inside are
/// felled/depleted; the meteorite crater cluster scatters within it. Kept below
/// the building clearance so the siting guarantee holds.
pub const METEOR_SHOWER_IMPACT_RADIUS_M: f32 = 18.0;

/// Radius, in metres, of the evacuation danger zone. A player whose own position
/// is inside this ring of the announced impact point gets the escalating
/// client-side "evacuate" warning over the final 60 seconds. Larger than the
/// impact radius so the warning gives players room to run clear rather than
/// firing only once they are already at ground zero.
pub const METEOR_SHOWER_DANGER_RADIUS_M: f32 = 60.0;

/// Blast damage applied at ground zero (distance 0 from the impact point). At
/// `MAX_HEALTH` this is lethal through any current armor set (the 60% cap still
/// leaves ~100 landing), matching the "standing on the marker is lethal" design.
/// Falls off linearly to 0 at `METEOR_SHOWER_IMPACT_RADIUS_M`.
pub const METEOR_SHOWER_IMPACT_PLAYER_DAMAGE: f32 = 250.0;

/// Minimum and maximum number of rich meteorite crater nodes the impact
/// scatters inside the crater. A contested windfall (several nodes' worth of the
/// rare iron-gated mineral in one spot) that despawns if unmined, so it rewards
/// rushing the site. Placed with a minimum spacing so they do not overlap.
pub const METEOR_SHOWER_CRATER_NODE_COUNT_MIN: u32 = 3;
pub const METEOR_SHOWER_CRATER_NODE_COUNT_MAX: u32 = 6;

/// Real seconds the crater and its crater cluster persist before the server
/// force-despawns any unmined crater nodes and cleans up the event (crater visual +
/// map marker removed client-side). Ten minutes: long enough to rush the site
/// and fight over the cluster, short enough that meteors striking while nobody
/// is online do not leave the world scattered with stale crater nodes and craters.
pub const METEOR_SHOWER_DESPAWN_SECONDS: f32 = 600.0;

/// Minimum spacing, in metres, between two scattered crater crater nodes, so the
/// cluster reads as several distinct nodes rather than one overlapping blob.
pub const METEOR_SHOWER_CRATER_NODE_SPACING_M: f32 = 2.5;

/// Real seconds the impact site burns after the strike (client-side particle
/// fires + their glow lights). Much shorter than the crater window: the fires
/// are the "something just hit here" read for the first minute or two, then
/// die out and leave only the scorch for the rest of the window.
pub const METEOR_SHOWER_SITE_FIRE_SECONDS: f32 = 100.0;

/// Real seconds, at the end of `METEOR_SHOWER_SITE_FIRE_SECONDS`, over which the
/// site fires ramp down (fewer/smaller flames, dimming light) rather than
/// cutting out at full blaze.
pub const METEOR_SHOWER_SITE_FIRE_FADE_SECONDS: f32 = 30.0;

/// Number of candidate impact points the site selector samples in the outer
/// ring before falling back to the max-clearance candidate. Rejection sampling
/// against the building/deployable/claim/ruin clearance keeps only safe sites;
/// this caps the search so a heavily-built map still resolves quickly.
pub const METEOR_SHOWER_SITE_CANDIDATES: u32 = 48;

/// Fraction of the playable radius the impact site must sit BEYOND (measured
/// from the world centre). Keeps meteors out to the committed outer reaches of
/// the map, the same "exploration lives outward" instinct the meteorite and
/// ruin rings use, and away from the central spawn area.
pub const METEOR_SHOWER_SITE_MIN_CENTER_DISTANCE_FRACTION: f32 = 0.35;

/// Margin, in metres, kept between the impact site and the edge of
/// `PlayableBounds` so the crater and its crater cluster never clip the world
/// perimeter wall.
pub const METEOR_SHOWER_SITE_BOUNDS_MARGIN_M: f32 = 20.0;

// =====================================================================
// Explosives
// =====================================================================
//
// The three blackpowder charges and the raid economics they drive. Two levers:
// a per-charge `*_BASE_DAMAGE` (blast damage at ground zero) and the
// effectiveness matrix `*_EFFECTIVENESS_<material>_PCT` (percent of base a
// charge deals against each raid material). A charge's damage to a structure is
// `base * effectiveness_pct / 100 * linear_falloff`. At point-blank the falloff
// is ~1.0, so the numbers below ARE the per-hit structure damage in the raid
// math. Wall/door HP (`BUILDING_*_WALL_HP`, `DOOR_MAX_HP`, `IRON_DOOR_MAX_HP`)
// are the targets these numbers are tuned against.
//
// Raid math (point-blank, one charge at the wall):
//   Hewn wood wall (3,600 HP): keg 900 * 80% = 720/charge -> 5 kegs (3,600) break
//     it, 4 (2,880) do not. Satchel 2,000 * 85% = 1,700/charge -> 2 satchels +
//     a bomb (300 * 40% = 120) also break it.
//   Stone wall (6,000 HP): satchel 2,000 * 45% = 900/charge -> 7 satchels
//     (6,300) break it.
//   Iron door (3,000 HP): with no dedicated metal charge the satchel.s 8%
//     (160/charge, ~19 satchels) is the only thing that touches metal at all;
//     an iron door is effectively raid-proof until a top-tier metal charge lands.
// Tune here, never inline.

/// Base blast damage at ground zero for each charge, before the per-material
/// effectiveness multiplier and the linear distance falloff. Straight from the
/// spec's "Base dmg" column (300 / 900 / 2,000).
pub const POWDER_BOMB_BASE_DAMAGE: u32 = 300;
pub const POWDER_KEG_BASE_DAMAGE: u32 = 900;
pub const SATCHEL_CHARGE_BASE_DAMAGE: u32 = 2_000;

/// Effectiveness matrix: percent of `base_damage` a charge deals against each
/// raid material, exactly the spec's per-charge row (Sticks / Wood / Stone /
/// Metal). Read by `explosive_effectiveness_pct`. `100` means full base damage;
/// `0` means the charge cannot touch that material at all (a powder bomb or keg
/// against an iron door).
pub const POWDER_BOMB_EFFECTIVENESS_STICKS_PCT: u32 = 100;
pub const POWDER_BOMB_EFFECTIVENESS_WOOD_PCT: u32 = 40;
pub const POWDER_BOMB_EFFECTIVENESS_STONE_PCT: u32 = 8;
pub const POWDER_BOMB_EFFECTIVENESS_METAL_PCT: u32 = 0;

pub const POWDER_KEG_EFFECTIVENESS_STICKS_PCT: u32 = 100;
pub const POWDER_KEG_EFFECTIVENESS_WOOD_PCT: u32 = 80;
pub const POWDER_KEG_EFFECTIVENESS_STONE_PCT: u32 = 25;
pub const POWDER_KEG_EFFECTIVENESS_METAL_PCT: u32 = 0;

pub const SATCHEL_CHARGE_EFFECTIVENESS_STICKS_PCT: u32 = 100;
pub const SATCHEL_CHARGE_EFFECTIVENESS_WOOD_PCT: u32 = 85;
pub const SATCHEL_CHARGE_EFFECTIVENESS_STONE_PCT: u32 = 45;
pub const SATCHEL_CHARGE_EFFECTIVENESS_METAL_PCT: u32 = 8;

/// Blast radius, in metres, for each charge. Full base damage at the centre,
/// falling off linearly to zero at this edge (both against structures and
/// players). Kept tight so a charge hits the wall it is set against and its
/// immediate neighbours, not a whole base. From the spec (~3.5 / 4 / 4).
pub const POWDER_BOMB_RADIUS_M: f32 = 3.5;
pub const POWDER_KEG_RADIUS_M: f32 = 4.0;
pub const SATCHEL_CHARGE_RADIUS_M: f32 = 4.0;

/// Fuse length, in server ticks, from arming to detonation. Placed charges
/// (keg / satchel) arm the instant they are set and hiss for 8 to 9 seconds,
/// the defender's window to defuse or shoot them out. The thrown bomb is lit
/// as it leaves the hand: the fuse counts through flight, bounce, and roll,
/// so it blows wherever it ends up ~4 seconds after the throw. Derived from
/// real seconds via the tick rate so the seconds are the single knob (the
/// `TORCH_BURN_TICKS` idiom).
pub const POWDER_KEG_FUSE_TICKS: u32 = (8.0 * SERVER_TICK_RATE_HZ) as u32;
pub const SATCHEL_CHARGE_FUSE_TICKS: u32 = (9.0 * SERVER_TICK_RATE_HZ) as u32;
/// The thrown powder bomb's fuse, counted from the moment it is thrown.
pub const POWDER_BOMB_FUSE_TICKS: u32 = (4.0 * SERVER_TICK_RATE_HZ) as u32;

/// HP of a placed charge. Deliberately small: a charge is a fizzleable target
/// (cloth material), so a defender can shoot or hit it a couple of times before
/// it blows to disarm the raid without a refund. Shared by all the charges.
pub const EXPLOSIVE_CHARGE_HP: u32 = 50;

/// Launch speed window, in metres per second, of a thrown powder bomb. The
/// throw charges like a bow draw: launch speed scales linearly with the held
/// charge fraction from the min (a short drop-toss at minimum charge) to the
/// max (a full wound-up lob). Heavier ballistics than an arrow (which flies at
/// 35+): a lobbed charge arcs and drops rather than shooting flat. Gravity is
/// the shared `PROJECTILE_GRAVITY`.
pub const POWDER_BOMB_MIN_THROW_SPEED_MPS: f32 = 6.0;
pub const POWDER_BOMB_MAX_THROW_SPEED_MPS: f32 = 16.0;

/// Seconds of held left-click to reach a full-power throw (the bow's
/// `draw_seconds` idiom). The charge clamps at 1.0 once reached; holding
/// longer changes nothing.
pub const POWDER_BOMB_CHARGE_SECONDS: f32 = 1.1;

/// Minimum charge fraction below which releasing the button cancels instead of
/// throwing, so a stray tap never lobs a bomb at your own feet (the
/// `BOW_MIN_DRAW_FRACTION_TO_FIRE` idiom). Enforced client-side (release
/// cancels) and clamped server-side (a forged lower power throws at this
/// fraction's speed, never slower).
pub const POWDER_BOMB_MIN_THROW_FRACTION: f32 = 0.25;

/// Bounce response of the thrown bomb, applied by the server projectile sim on
/// each solid contact: the normal component of the velocity reflects scaled by
/// the restitution, the tangential component keeps rolling scaled by the
/// friction. Together they read as "lands, bounces once or twice, rolls to a
/// stop" rather than sticking to the first surface it touches.
pub const POWDER_BOMB_RESTITUTION: f32 = 0.42;
pub const POWDER_BOMB_BOUNCE_FRICTION: f32 = 0.72;

/// Below this speed (m/s) after a ground contact the bomb stops simulating and
/// rests in place (still fused). Keeps the tail of the roll from micro-jittering
/// forever on integrator noise.
pub const POWDER_BOMB_REST_SPEED_MPS: f32 = 1.1;

/// Radius of the powder bomb's cloth ball (the glb ball spans y 0..0.25; the
/// fuse cap above it deliberately does NOT collide). The server sim sweeps a
/// SPHERE of this radius (ground plane lifted by it, block/deployable AABBs
/// inflated by it), so the replicated position is the BALL CENTER and the
/// bomb rolls smoothly on its ball instead of pivoting on the mesh base. The
/// client sinks the mesh by this much under the visual root and rolls about
/// it at `speed / radius`.
pub const POWDER_BOMB_BALL_RADIUS_M: f32 = 0.125;

/// Knockback impulse magnitude, in m/s, an explosion applies to a player at
/// ground zero (scaled down by the linear falloff toward the edge). A single
/// knob shared by every charge; the direction is radial, away from the blast.
pub const EXPLOSION_KNOCKBACK_SPEED: f32 = 10.0;

/// Range, in metres, within which the cosmetic `ServerMessage::Explosion` VFX/
/// SFX cue is fanned out to clients. Generous (well past any blast radius) so a
/// player hears and sees a distant breach, the audible-thump-plus-far-rumble
/// feel; the client scales the effect by its own distance to the blast.
pub const EXPLOSION_CUE_RANGE_M: f32 = 120.0;

/// Reach, in metres, within which a defender may hold-E defuse a placed charge.
/// Matches the general deployable interaction reach (`DEPLOYABLE_PLACEMENT_REACH_M`)
/// so walking up to a hissing charge and defusing it uses the same distance the
/// rest of the deployable interactions do (measured to the charge's collider
/// surface).
pub const EXPLOSIVE_DEFUSE_REACH_M: f32 = 5.0;

/// Numerator/denominator of the recipe materials a successful defuse refunds to
/// the defender, per material and rounded down: half the charge back (`1/2`).
/// The defuser recovers materials, but never all of them, so defusing still
/// costs the raider real farm time even when countered, and the defender is not
/// fully reimbursed for a charge they did not craft. Kept as an integer ratio so
/// the refund math stays exact (`input.quantity * NUM / DEN`, floor).
pub const EXPLOSIVE_DEFUSE_REFUND_NUMERATOR: u16 = 1;
pub const EXPLOSIVE_DEFUSE_REFUND_DENOMINATOR: u16 = 2;

// =====================================================================
// CONSUMABLES (healing)
// =====================================================================
// The bandage is the game's first consumable and its only healing outside a
// respawn. It is deliberately cheap and abundant (see the recipe: cloth + fiber,
// both hand-gathered from tall grass) because it is not meant to be a scarce
// resource to hoard. It is meant to be a *tempo* decision: the charge is long
// enough that you cannot use one in the middle of a fight and survive, and the
// movement slow means committing to it in the open is how you get killed.
//
// Total per bandage is 35 HP, of which under half lands immediately. That split
// is the whole design: the instant chunk stops a bleed-out, but you only bank
// the full value if you actually break contact.

/// Bandage: ticks the use must be held before it applies (3 s at 20 Hz). Long on
/// purpose. Shorter than this and a bandage becomes a mid-melee panic button
/// that erases a landed hit; at 3 s you have to disengage first, which is the
/// decision the item is supposed to pose.
pub const BANDAGE_USE_TICKS: u64 = (3.0 * SERVER_TICK_RATE_HZ) as u64;
/// Bandage: health restored the instant the wrap completes. Enough that finishing
/// the charge has a felt payoff and pulls you off the floor, well short of a full
/// reset.
pub const BANDAGE_INSTANT_HEAL: f32 = 15.0;
/// Bandage: additional health trickled in over `BANDAGE_HEAL_DURATION_TICKS`
/// after it applies. The larger half of the item's value, and the half you lose
/// if you walk straight back into a fight.
pub const BANDAGE_HEAL_OVER_TIME: f32 = 20.0;
/// Bandage: how long the over-time remainder takes to fully land (10 s at 20 Hz).
/// Long enough that re-engaging immediately forfeits most of it.
pub const BANDAGE_HEAL_DURATION_TICKS: u64 = (10.0 * SERVER_TICK_RATE_HZ) as u64;
/// Bandage: run-speed multiplier while the use is being charged. Harsher than the
/// bow's draw slow (0.6): binding a wound is a full commitment, and you should not
/// be able to back-pedal out of a fight at near-full speed while doing it.
pub const BANDAGE_USE_MOVE_MULTIPLIER: f32 = 0.4;
/// Bandage: max stack. Small, so carrying a meaningful amount of healing costs
/// real inventory slots and a bandage stack is a visible commitment in a loot bag.
pub const BANDAGE_STACK_SIZE: u16 = 5;
