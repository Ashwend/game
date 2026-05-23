use bevy::prelude::*;

use crate::app::state::{ClientSettings, InventoryUiState, KeyAction, MenuState, Screen};

pub(crate) fn chat_shortcut_system(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<ClientSettings>,
    mut menu: ResMut<MenuState>,
) {
    if menu.screen != Screen::InGame || menu.pause_open || menu.chat_open {
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
    }
}

pub(crate) fn toggle_inventory_system(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<ClientSettings>,
    mut menu: ResMut<MenuState>,
    mut inventory_ui: ResMut<InventoryUiState>,
) {
    if menu.screen != Screen::InGame || menu.pause_open || menu.pause_options_open || menu.chat_open
    {
        return;
    }

    if settings
        .keybindings
        .just_pressed(KeyAction::OpenInventory, &keys)
    {
        menu.inventory_open = !menu.inventory_open;
        if !menu.inventory_open {
            inventory_ui.cancel_drag();
        }
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
