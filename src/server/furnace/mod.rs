//! Server-authoritative furnace state + smelt loop + open/close
//! interaction commands.
//!
//! Furnaces are entity-bound, not recipe-bound: smelting happens inside
//! the furnace's own inventory grid rather than through the regular
//! crafting registry. Each tick the head smeltable input advances by
//! one tick; on completion the input stack shrinks by one and the
//! matching output stack grows by one. Fuel ticks down on every tick
//! the furnace is actively smelting. Auto-shutoff fires when the
//! output won't fit, when fuel runs out mid-smelt, or when there's
//! nothing left to smelt.
//!
//! Module layout:
//!   - [`state`]: `FurnaceState`, persistence shims, constants, and
//!     pure helpers that don't touch `GameServer`. Unit-testable
//!     without spinning up a server.
//!   - [`tick`]: the per-tick smelt loop + the `GameServer::tick_furnaces`
//!     entry point.
//!   - [`commands`]: the `GameServer::apply_furnace_command` dispatcher
//!     plus all `Open`/`Close`/`Move`/`QuickTransfer` handlers. Every
//!     command path re-validates the player's distance to the open
//!     furnace so a client whose UI persisted after they walked away
//!     can't move items out of line-of-sight.

mod commands;
mod state;
mod tick;

pub(crate) use state::FurnaceState;

#[cfg(test)]
pub(crate) use state::{SMELT_TICKS_PER_OUTPUT, WOOD_BURN_TICKS, merge_into_optional_slot};
#[cfg(test)]
pub(crate) use tick::tick_one_furnace;
