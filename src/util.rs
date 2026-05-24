//! Tiny cross-module helpers. Keep this module dependency-light — anything
//! in here should be reachable from `protocol`, `controller`, `server`, and
//! the client tree without pulling in heavy crates.

pub mod hash;
pub mod variation;
