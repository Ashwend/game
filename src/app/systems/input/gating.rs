use bevy::{prelude::*, window::PrimaryWindow};

use crate::app::state::{MenuState, Screen};

/// True if local game simulation should run this frame (network ticks,
/// prediction). Stops only on screens that suspend gameplay outright —
/// pause-options and chat overlays — not on plain pause or inventory.
pub(super) fn gameplay_simulation_allowed(menu: &MenuState) -> bool {
    menu.screen == Screen::InGame && !menu.pause_options_open && !menu.chat_open
}

/// True if the local player should accept movement/look/swing controls.
/// Stricter than `gameplay_simulation_allowed` — the window must be focused
/// and no modal UI (pause menu, inventory) can be in the way.
pub(super) fn gameplay_accepts_controls(menu: &MenuState, window_focused: bool) -> bool {
    window_focused
        && gameplay_simulation_allowed(menu)
        && !menu.pause_open
        && !menu.inventory_open
        && !menu.crafting_open
        && !menu.furnace_open
}

pub(super) fn primary_window_focused(primary_window: &Query<&Window, With<PrimaryWindow>>) -> bool {
    primary_window
        .single()
        .map(|window| window.focused)
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfocused_gameplay_blocks_controls_without_blocking_simulation() {
        let menu = MenuState {
            screen: Screen::InGame,
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(gameplay_accepts_controls(&menu, true));
        assert!(!gameplay_accepts_controls(&menu, false));
    }

    #[test]
    fn pause_options_block_gameplay_simulation_and_controls() {
        let menu = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            pause_options_open: true,
            ..Default::default()
        };

        assert!(!gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_controls(&menu, true));
    }

    #[test]
    fn pause_menu_blocks_controls_without_blocking_gameplay_simulation() {
        let menu = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_controls(&menu, true));
    }

    #[test]
    fn inventory_blocks_controls_without_blocking_simulation() {
        let menu = MenuState {
            screen: Screen::InGame,
            inventory_open: true,
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_controls(&menu, true));
    }
}
