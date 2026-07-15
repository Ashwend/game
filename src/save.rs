//! World save persistence, atomic file writes, binary format codec,
//! listing/recovery, and name validation. Submodules:
//!
//! - [`store`], `WorldStore`, the directory-rooted entry point
//! - [`mod@format`], `GAMESAVE` magic, format version, encode/decode
//! - [`types`], `WorldSave`, `WorldStateSave`, `PersistedPlayer`
//! - [`listing`], listing results, corrupted-file recovery
//! - [`validate`], player-supplied world-name validation

mod format;
mod listing;
mod store;
mod types;
mod validate;

pub use format::{load_world_file, save_world_file};
pub use listing::{CorruptedWorld, WorldListing, WorldSummary};
pub use store::WorldStore;
pub use types::{
    PersistedAccountMarkers, PersistedCupboardState, PersistedDeployedEntity, PersistedDoorState,
    PersistedFurnaceState, PersistedFuseState, PersistedPlayer, PersistedRuinCacheState,
    PersistedStorageBoxState, PersistedTorchState, WorldSave, WorldStateSave,
};
pub use validate::{MAX_WORLD_NAME_LEN, validate_world_name};
