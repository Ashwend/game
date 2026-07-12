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

/// True when no modal UI other than the world map is in the way. The shared
/// core of the two gates below: every overlay that should freeze input EXCEPT
/// the navigable world map (which only freezes look/swing, not movement).
fn no_blocking_modal(menu: &MenuState) -> bool {
    !menu.pause_open
        && !menu.inventory_open
        && !menu.crafting_open
        && !menu.furnace_open
        && !menu.loot_bag_open
        && !menu.workbench_open
        && !menu.chat_open
        // Single-field text dialogs (door codes, bag rename, marker name),
        // confirm modals (e.g. marker delete), and notices all own the screen;
        // gameplay controls stay frozen so a keystroke or click meant for the
        // dialog doesn't also drive the player behind it.
        && !menu.dialog_modal_open()
        // Dead players can move the cursor over the respawn button,
        // gameplay controls stay frozen until the respawn lands.
        && menu.death_splash.is_none()
        // The world-entry loading splash owns the screen while the initial
        // world streams in. The player is nominally in-game underneath it
        // (simulation keeps ticking, per the invariant), but no look, swing,
        // or movement input may leak through the opaque overlay: the player
        // can't see what they'd be doing.
        && !menu.world_entry_splash_active()
}

/// True if the local player should accept look/swing/cursor-capture controls.
/// Stricter than `gameplay_simulation_allowed`, the window must be focused
/// and no modal UI (pause menu, inventory, chat, loot bag, death splash, or
/// the world map) can be in the way. The world map frees the cursor so the
/// player can click its markers, so look/swing stay gated while it's open.
pub(super) fn gameplay_accepts_controls(menu: &MenuState, window_focused: bool) -> bool {
    window_focused
        && gameplay_simulation_allowed(menu)
        // The world map is a navigable overlay: it frees the cursor (for
        // marker interaction) and freezes look/swing, but NOT movement, see
        // `gameplay_accepts_movement`.
        && !menu.world_map_open
        && no_blocking_modal(menu)
}

/// True if the local player should accept WASD movement input. Identical to
/// `gameplay_accepts_controls` except the world map does not block it: the map
/// is a navigable overlay, so the player can keep running (to check their
/// coordinates against the map) while it's open. Look, swing, and the cursor
/// grab stay gated through `gameplay_accepts_controls`.
pub(super) fn gameplay_accepts_movement(menu: &MenuState, window_focused: bool) -> bool {
    window_focused && gameplay_simulation_allowed(menu) && no_blocking_modal(menu)
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
    fn world_map_blocks_look_but_not_movement_or_simulation() {
        // The map is a navigable overlay: it freezes look/swing (cursor is
        // freed for marker interaction) but keeps WASD live so the player can
        // run while checking their coordinates, and, like every overlay, must
        // never halt the authoritative simulation underneath.
        let menu = MenuState {
            screen: Screen::InGame,
            world_map_open: true,
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_controls(&menu, true));
        assert!(gameplay_accepts_movement(&menu, true));
        // An unfocused window still freezes everything, even movement.
        assert!(!gameplay_accepts_movement(&menu, false));
    }

    #[test]
    fn a_real_modal_over_the_map_freezes_movement_too() {
        // If a text prompt (e.g. naming a marker) opens on top of the map,
        // movement must freeze: the keyboard belongs to the text field.
        let menu = MenuState {
            screen: Screen::InGame,
            world_map_open: true,
            text_prompt: Some(crate::app::state::TextPrompt::new(
                crate::app::state::TextPromptKind::NameWorldMapMarker { id: 1 },
            )),
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_movement(&menu, true));
    }

    #[test]
    fn an_in_game_confirm_dialog_freezes_controls_and_movement() {
        // A confirm modal (e.g. the marker-delete confirm) owns the keyboard +
        // pointer; gameplay controls AND movement must freeze so a keystroke or
        // click meant for the dialog doesn't also drive the player behind it.
        // Simulation, as always, keeps running.
        let menu = MenuState {
            screen: Screen::InGame,
            confirmation: Some(
                crate::app::state::ConfirmationDialog::delete_world_map_marker(1, "base"),
            ),
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_controls(&menu, true));
        assert!(!gameplay_accepts_movement(&menu, true));
    }

    #[test]
    fn world_entry_loading_splash_blocks_controls_and_movement() {
        // While the loading splash streams the world in, the screen is
        // already InGame underneath it (so simulation runs), but no input
        // may leak through the opaque overlay: the player can't see the
        // world they'd be driving.
        let menu = MenuState {
            screen: Screen::InGame,
            loading_splash: Some(crate::app::state::LoadingSplash::new(
                crate::app::state::LoadingSplashKind::EnteringWorld,
                "World",
            )),
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_controls(&menu, true));
        assert!(!gameplay_accepts_movement(&menu, true));
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
