use bevy::prelude::*;

use crate::app::state::{
    ClientSettings, CraftingUiState, InventoryUiState, KeyAction, MenuState, Screen,
};

/// Hardcoded F2 toggle for the performance overlay. Not rebindable in the
/// keybind UI — the FPS/perf overlay sits in the "debug toggles" bucket
/// where a fixed key is easier than a configurable one.
pub(crate) fn toggle_perf_stats_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut settings: ResMut<ClientSettings>,
    menu: Res<MenuState>,
) {
    if menu.screen != Screen::InGame || menu.pause_open || menu.chat_open {
        return;
    }
    if keys.just_pressed(KeyCode::F2) {
        settings.hud.show_perf_stats = !settings.hud.show_perf_stats;
    }
}

pub(crate) fn chat_shortcut_system(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<ClientSettings>,
    mut menu: ResMut<MenuState>,
) {
    // Any other modal screen suppresses the chat hotkey:
    //  - `pause_open` / `chat_open`: existing gates.
    //  - `crafting_open`: the crafting search is a text input. Letting
    //    `T` fire chat here would steal focus to the chat input the
    //    moment the player typed a `t` into the search.
    //  - `inventory_open`: opening chat on top of the inventory makes a
    //    visual mess; if the player wants chat they can close the bag
    //    first.
    if menu.screen != Screen::InGame
        || menu.pause_open
        || menu.chat_open
        || menu.crafting_open
        || menu.inventory_open
    {
        return;
    }

    if settings
        .keybindings
        .just_pressed(KeyAction::OpenChat, &keys)
    {
        menu.chat_open = true;
        menu.chat_focus_pending = true;
        menu.chat_input.clear();
    }
}

pub(crate) fn toggle_pause_system(keys: Res<ButtonInput<KeyCode>>, mut menu: ResMut<MenuState>) {
    if menu.screen != Screen::InGame {
        return;
    }
    if menu.chat_open {
        return;
    }

    if keys.just_pressed(KeyCode::Escape) {
        handle_pause_escape(&mut menu);
    }
}

fn handle_pause_escape(menu: &mut MenuState) {
    if menu.inventory_open {
        menu.inventory_open = false;
        return;
    }

    if menu.crafting_open {
        menu.crafting_open = false;
        return;
    }

    if menu.pause_options_open {
        menu.pause_open = true;
        menu.pause_options_open = false;
        return;
    }

    menu.pause_open = !menu.pause_open;
    if !menu.pause_open {
        menu.pause_options_open = false;
    } else {
        menu.inventory_open = false;
        menu.crafting_open = false;
    }
}

pub(crate) fn toggle_inventory_system(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<ClientSettings>,
    mut menu: ResMut<MenuState>,
    mut inventory_ui: ResMut<InventoryUiState>,
) {
    // We *do* let Tab fire while the crafting modal is up — it's the
    // convenient "swap to inventory" gesture players asked for. The
    // crafting search input swallows the tab character itself (egui
    // never types a literal `\t`), so the only visible effect is the
    // screen swap below.
    if menu.screen != Screen::InGame || menu.pause_open || menu.pause_options_open || menu.chat_open
    {
        return;
    }

    if settings
        .keybindings
        .just_pressed(KeyAction::OpenInventory, &keys)
    {
        menu.inventory_open = !menu.inventory_open;
        if menu.inventory_open {
            // Inventory and crafting share the same cursor-unlock state.
            // Opening one closes the other so they don't overlap.
            menu.crafting_open = false;
        } else {
            inventory_ui.cancel_drag();
        }
    }
}

pub(crate) fn toggle_crafting_system(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<ClientSettings>,
    mut menu: ResMut<MenuState>,
    mut inventory_ui: ResMut<InventoryUiState>,
    mut crafting_ui: ResMut<CraftingUiState>,
) {
    // C only *opens* the screen — closing is via Escape (handled by
    // `handle_pause_escape`) or clicking the backdrop. Without this,
    // typing `c` into the focused search box would close the menu, and
    // there's no reliable way from a `Update`-phase Bevy system to know
    // whether egui currently owns the keyboard. The chat box uses the
    // same "open with hotkey, close with Escape" pattern.
    if menu.screen != Screen::InGame
        || menu.pause_open
        || menu.pause_options_open
        || menu.chat_open
        || menu.crafting_open
    {
        return;
    }

    if settings
        .keybindings
        .just_pressed(KeyAction::OpenCrafting, &keys)
    {
        menu.crafting_open = true;
        menu.inventory_open = false;
        inventory_ui.cancel_drag();
        // Drop the player straight into the search box — most players
        // open this screen to find a specific recipe — and start them
        // from an empty query so stale filters from a previous session
        // don't hide the full recipe list.
        crafting_ui.search.clear();
        crafting_ui.focus_search_pending = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_closes_pause_options_back_to_pause_menu() {
        let mut menu = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            pause_options_open: true,
            ..Default::default()
        };

        handle_pause_escape(&mut menu);

        assert!(menu.pause_open);
        assert!(!menu.pause_options_open);
    }

    #[test]
    fn escape_toggles_pause_root_and_clears_nested_options_when_closed() {
        let mut menu = MenuState {
            screen: Screen::InGame,
            ..Default::default()
        };

        handle_pause_escape(&mut menu);
        assert!(menu.pause_open);

        menu.pause_options_open = true;
        handle_pause_escape(&mut menu);
        assert!(menu.pause_open);
        assert!(!menu.pause_options_open);

        handle_pause_escape(&mut menu);
        assert!(!menu.pause_open);
        assert!(!menu.pause_options_open);
    }
}
