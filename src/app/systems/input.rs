//! Client input pipeline. Split into focused submodules:
//!
//! - `gating` — `gameplay_simulation_allowed`/`gameplay_accepts_controls`/
//!   `primary_window_focused`; the gating logic the others lean on.
//! - `cursor` — cursor grab and focus-recentering.
//! - `menu_toggles` — chat/pause/inventory shortcut keys.
//! - `look` — mouse look + per-frame delta cap.
//! - `movement` — `client_input_system` and the WASD→direction helper.
//! - `inventory_shortcuts` — actionbar, drop/pickup, swing dispatch.

mod cursor;
mod gating;
mod inventory_shortcuts;
mod look;
mod menu_toggles;
mod movement;

pub(crate) use cursor::{center_cursor_on_focus_system, update_cursor_system};
pub(crate) use inventory_shortcuts::{gameplay_inventory_shortcuts_system, send_inventory_command};
pub(crate) use look::mouse_look_system;
pub(crate) use menu_toggles::{
    chat_shortcut_system, toggle_inventory_system, toggle_pause_system, toggle_perf_stats_system,
};
pub(crate) use movement::client_input_system;
