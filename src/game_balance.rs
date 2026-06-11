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

/// Cosine of the attacker's view-cone half-angle.
pub const COMBAT_ATTACK_CONE_COS: f32 = 0.92;

/// Vertical offset from the attacker's feet to the eye, used as the
/// LOS ray origin server-side.
pub const COMBAT_ATTACKER_EYE_HEIGHT: f32 = 1.62;

/// Chest-height aim point relative to the target's feet, used as the
/// LOS ray's destination.
pub const COMBAT_TARGET_CHEST_HEIGHT: f32 = 0.95;

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

/// Sleeping bag HP. Cloth tears fast; bags are respawn anchors, not cover.
pub const SLEEPING_BAG_MAX_HP: u32 = 100;

/// Storage box HP (plain Wood material, so any proper tool opens one up
/// eventually). Boxes are loot pinatas by design: keeping valuables safe
/// is what walls and doors are for.
pub const STORAGE_BOX_SMALL_HP: u32 = 400;
pub const STORAGE_BOX_LARGE_HP: u32 = 700;

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

/// Hammer durability: same budget as an iron tool, repairs and upgrades
/// are frequent but cheap taps.
pub const HAMMER_DURABILITY: u32 = 600;

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
