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

/// When choosing a safe respawn spot, no other live player may be
/// closer than this distance. Prevents respawn camping.
pub const RESPAWN_MIN_DISTANCE_M: f32 = 12.0;

// =====================================================================
// Deployables (workbenches, furnaces, walls, …)
// =====================================================================

/// Maximum distance at which a player can damage a placed structure.
/// Matches the furnace open-range so the swing flow stays consistent,
/// if E reaches it, your tool reaches it too.
pub const DEPLOYABLE_DAMAGE_RANGE_M: f32 = 5.5;

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

/// Maximum interaction range, in metres, for `E`-to-open. Slightly
/// larger than `DEPLOYABLE_PLACEMENT_REACH_M` so a player who placed
/// at max reach can still interact without having to step forward.
pub const FURNACE_INTERACT_RANGE_M: f32 = 5.5;

// =====================================================================
// Loot bags
// =====================================================================

/// Maximum interaction range, in metres, for opening a loot bag. The
/// drop-on-death bag is meant to be approached, not looted from
/// across the room.
pub const LOOT_BAG_INTERACT_RANGE_M: f32 = 4.5;
