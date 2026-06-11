use bevy::{prelude::*, window::PrimaryWindow};

use crate::app::state::{MenuState, Screen};

/// True if local game simulation should run this frame (network ticks,
/// prediction). Per CLAUDE.md's "gameplay never pauses" invariant, the
/// only thing that halts simulation is leaving the in-game screen
/// entirely. Every in-game overlay, pause, pause-options, inventory,
/// chat, crafting, furnace, death splash, keeps the simulator
/// ticking so server-pushed effects (knockback, replication, deaths
/// landing while the menu is open) all keep applying in real time.
/// Overlays only gate local input via `gameplay_accepts_controls`
/// below.
pub(super) fn gameplay_simulation_allowed(menu: &MenuState) -> bool {
    menu.screen == Screen::InGame
}

/// True if the local player should accept movement/look/swing controls.
/// Stricter than `gameplay_simulation_allowed`, the window must be focused
/// and no modal UI (pause menu, inventory, chat, loot bag, death splash)
/// can be in the way.
pub(super) fn gameplay_accepts_controls(menu: &MenuState, window_focused: bool) -> bool {
    window_focused
        && gameplay_simulation_allowed(menu)
        && !menu.pause_open
        && !menu.inventory_open
        && !menu.crafting_open
        && !menu.furnace_open
        && !menu.loot_bag_open
        && !menu.chat_open
        // Single-field text dialogs (door codes, bag rename) capture the
        // keyboard; gameplay controls stay frozen while one is up.
        && menu.text_prompt.is_none()
        // Dead players can move the cursor over the respawn button,
        // gameplay controls (WASD, mouse-look, swing) stay frozen
        // until the respawn lands.
        && menu.death_splash.is_none()
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
    fn pause_options_blocks_controls_without_blocking_simulation() {
        // Gameplay never pauses, even while the player is editing
        // settings: the authoritative server keeps running and the
        // local client must keep ticking to stay in sync. Only
        // input is gated.
        let menu = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            pause_options_open: true,
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
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

    #[test]
    fn chat_blocks_controls_without_blocking_simulation() {
        // PvP-driven invariant: knockback impulses arrive through the
        // network tick and must integrate into the predictor even when
        // chat is open, otherwise the velocity accumulates and fires
        // off the moment chat closes. Mirrors the same split that
        // inventory/pause already use.
        let menu = MenuState {
            screen: Screen::InGame,
            chat_open: true,
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_controls(&menu, true));
    }
}
