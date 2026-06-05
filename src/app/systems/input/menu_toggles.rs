use bevy::prelude::*;

use crate::app::state::{
    ClientSettings, CraftingUiState, InventoryUiState, KeyAction, MenuState, Screen,
};

/// Hardcoded F2 toggle for the performance overlay. Not rebindable in the
/// keybind UI, the FPS/perf overlay sits in the "debug toggles" bucket
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
    // Chat visibility is independent of the HUD master toggle, so only the
    // chat toggle gates it here. If chat is hidden but something opened it
    // anyway, force it closed: an invisible open chat would strand
    // movement/look controls behind `chat_open` with no way to dismiss it
    // (Escape leaves chat to its own input box, which isn't drawn).
    if !settings.hud.show_chat && menu.chat_open {
        menu.chat_open = false;
        menu.chat_focus_pending = false;
        return;
    }

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
        // Dead players can't chat, loot, or do anything but click
        // Respawn. The splash is the only modal on screen.
        || menu.death_splash.is_some()
        // Chat hidden for screenshots: don't let the hotkey open an
        // invisible chat box that would silently swallow controls. Gated on
        // the chat toggle alone, the HUD master doesn't affect chat.
        || !settings.hud.show_chat
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

pub(crate) fn toggle_pause_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut menu: ResMut<MenuState>,
    mut update: ResMut<crate::update::UpdateState>,
) {
    if menu.screen != Screen::InGame {
        return;
    }
    let escape = keys.just_pressed(KeyCode::Escape);

    // ESC closes the update / changelog modal first if it's open, whether it was
    // opened from the corner pill (no pause) or the pause menu's update row.
    if escape && update.modal_open {
        update.modal_open = false;
        return;
    }

    if menu.chat_open {
        return;
    }
    // ESC during the death splash is a no-op, the player has one
    // exit (the Respawn button), not a pause menu hop.
    if menu.death_splash.is_some() {
        return;
    }

    if escape {
        handle_pause_escape(&mut menu);
    }
}

fn handle_pause_escape(menu: &mut MenuState) {
    // Furnace + loot bag are both handled by their own
    // close-on-escape systems because the close path is a network
    // round-trip (server clears the open-state field, replication
    // mirrors it back). Bailing here means ESC stops at "close the
    // bag" instead of falling through to open the pause menu.
    if menu.furnace_open || menu.loot_bag_open {
        return;
    }

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

/// Mirror the local player's replicated `PlayerPrivate.open_furnace`
/// onto `MenuState.furnace_open` so the input-gating helpers can
/// suppress movement/look/swing without having to peek into the
/// replicated component themselves.
pub(crate) fn sync_furnace_open_flag_system(
    local_player: Res<crate::app::state::LocalPlayerState>,
    mut menu: ResMut<MenuState>,
) {
    let open = local_player
        .private
        .as_ref()
        .and_then(|private| private.open_furnace.as_ref())
        .is_some();
    if menu.furnace_open != open {
        menu.furnace_open = open;
    }
}

/// Same as `sync_furnace_open_flag_system` but for the loot bag.
/// Drives `MenuState.loot_bag_open` off the replicated
/// `PlayerPrivate.open_loot_bag`.
pub(crate) fn sync_loot_bag_open_flag_system(
    local_player: Res<crate::app::state::LocalPlayerState>,
    mut menu: ResMut<MenuState>,
) {
    let open = local_player
        .private
        .as_ref()
        .and_then(|private| private.open_loot_bag.as_ref())
        .is_some();
    if menu.loot_bag_open != open {
        menu.loot_bag_open = open;
    }
}

/// Send a loot bag `Close` command when ESC is pressed while the
/// bag is open. Mirror of `close_furnace_on_escape_system`.
pub(crate) fn close_loot_bag_on_escape_system(
    keys: Res<ButtonInput<KeyCode>>,
    menu: Res<MenuState>,
    mut runtime: ResMut<crate::app::state::ClientRuntime>,
    mut error_toasts: MessageWriter<crate::app::state::ClientErrorToast>,
) {
    if menu.screen != Screen::InGame || !menu.loot_bag_open {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        crate::app::systems::input::send_loot_bag_command(
            &mut runtime,
            &mut error_toasts,
            crate::protocol::LootBagCommand::Close,
        );
    }
}

/// Send a furnace `Close` command when ESC is pressed while the furnace
/// modal is open. Kept separate from `toggle_pause_system` so the
/// authority round-trip (client → server → snapshot clears
/// `open_furnace` → client mirrors `furnace_open = false`) doesn't tangle
/// with the local-only pause/inventory toggle logic.
pub(crate) fn close_furnace_on_escape_system(
    keys: Res<ButtonInput<KeyCode>>,
    menu: Res<MenuState>,
    mut runtime: ResMut<crate::app::state::ClientRuntime>,
    mut error_toasts: MessageWriter<crate::app::state::ClientErrorToast>,
) {
    if menu.screen != Screen::InGame || !menu.furnace_open {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        crate::app::systems::input::send_furnace_command(
            &mut runtime,
            &mut error_toasts,
            crate::protocol::FurnaceCommand::Close,
        );
    }
}

pub(crate) fn toggle_inventory_system(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<ClientSettings>,
    mut menu: ResMut<MenuState>,
    mut inventory_ui: ResMut<InventoryUiState>,
    mut runtime: ResMut<crate::app::state::ClientRuntime>,
    mut error_toasts: MessageWriter<crate::app::state::ClientErrorToast>,
) {
    // We *do* let Tab fire while the crafting modal is up - it's the
    // convenient "swap to inventory" gesture players asked for. The
    // crafting search input swallows the tab character itself (egui
    // never types a literal `\t`), so the only visible effect is the
    // screen swap below.
    if menu.screen != Screen::InGame
        || menu.pause_open
        || menu.pause_options_open
        || menu.chat_open
        || menu.death_splash.is_some()
    {
        return;
    }

    if settings
        .keybindings
        .just_pressed(KeyAction::OpenInventory, &keys)
    {
        menu.inventory_open = !menu.inventory_open;
        if menu.inventory_open {
            // Inventory takes over the cursor; close every other modal
            // we don't want layered behind it. The furnace + loot bag
            // close via a network round-trip because their open state
            // lives server-side, sending Close now means the next
            // replication tick clears the matching `*_open` flag on
            // its own.
            menu.crafting_open = false;
            if menu.furnace_open {
                close_open_furnace(&mut runtime, &mut error_toasts);
            }
            if menu.loot_bag_open {
                close_open_loot_bag(&mut runtime, &mut error_toasts);
            }
        } else {
            inventory_ui.cancel_drag();
        }
    }
}

/// Send a furnace `Close` command. Used by inventory/crafting open
/// handlers so opening either modal doesn't leave a furnace UI
/// running underneath the new view.
fn close_open_furnace(
    runtime: &mut crate::app::state::ClientRuntime,
    error_toasts: &mut MessageWriter<crate::app::state::ClientErrorToast>,
) {
    crate::app::systems::input::send_furnace_command(
        runtime,
        error_toasts,
        crate::protocol::FurnaceCommand::Close,
    );
}

/// Same idea for the loot bag, opening inventory / crafting / a new
/// modal shouldn't leave a bag UI ghosted behind the new view.
fn close_open_loot_bag(
    runtime: &mut crate::app::state::ClientRuntime,
    error_toasts: &mut MessageWriter<crate::app::state::ClientErrorToast>,
) {
    crate::app::systems::input::send_loot_bag_command(
        runtime,
        error_toasts,
        crate::protocol::LootBagCommand::Close,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn toggle_crafting_system(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<ClientSettings>,
    mut menu: ResMut<MenuState>,
    mut inventory_ui: ResMut<InventoryUiState>,
    mut crafting_ui: ResMut<CraftingUiState>,
    mut contexts: bevy_egui::EguiContexts,
    mut runtime: ResMut<crate::app::state::ClientRuntime>,
    mut error_toasts: MessageWriter<crate::app::state::ClientErrorToast>,
) {
    // Same hotkey opens and closes the screen, but a closing press has
    // to be careful not to fight a focused egui text input. We check
    // `wants_keyboard_input` so typing `c` into the search box leaves
    // the modal open (the input absorbs the keystroke), while pressing
    // `C` with the search box unfocused toggles the modal off.
    if menu.screen != Screen::InGame
        || menu.pause_open
        || menu.pause_options_open
        || menu.chat_open
        || menu.death_splash.is_some()
    {
        return;
    }

    if !settings
        .keybindings
        .just_pressed(KeyAction::OpenCrafting, &keys)
    {
        return;
    }

    // We need to know whether the player is currently typing into the
    // crafting search field, so a `c` typed in the search doesn't get
    // hijacked as "close the modal". `wants_keyboard_input` is too
    // broad - egui's Tab focus-cycling lands focus on the first
    // interactive widget when the inventory opens (it's a slot, not a
    // text input), and that made `wants_keyboard_input` true and broke
    // the C hotkey until the player clicked elsewhere. Pin the check to
    // the search input's id so only that specific case bails.
    let crafting_search_focused = contexts
        .ctx_mut()
        .map(|ctx| {
            ctx.memory(|memory| {
                memory.focused() == Some(bevy_egui::egui::Id::new("crafting_search_input"))
            })
        })
        .unwrap_or(false);
    if crafting_search_focused {
        return;
    }

    if menu.crafting_open {
        menu.crafting_open = false;
        return;
    }

    open_crafting_modal(
        &mut menu,
        &mut inventory_ui,
        &mut crafting_ui,
        &mut runtime,
        &mut error_toasts,
    );
}

/// Open the crafting modal and reset its transient browser state.
/// Shared between the C hotkey and the "press E on a workbench" path
/// so both entry points behave identically (clear search, scroll to
/// top, close any furnace that was up, don't auto-focus the input).
pub(crate) fn open_crafting_modal(
    menu: &mut MenuState,
    inventory_ui: &mut InventoryUiState,
    crafting_ui: &mut CraftingUiState,
    runtime: &mut crate::app::state::ClientRuntime,
    error_toasts: &mut MessageWriter<crate::app::state::ClientErrorToast>,
) {
    menu.crafting_open = true;
    menu.inventory_open = false;
    inventory_ui.cancel_drag();
    if menu.furnace_open {
        // The furnace lives server-side so we ship a Close and the
        // next snapshot will clear our mirrored `furnace_open` flag.
        close_open_furnace(runtime, error_toasts);
    }
    if menu.loot_bag_open {
        // Same lifecycle as the furnace: server-side state, close
        // round-trip clears the local mirror on the next tick.
        close_open_loot_bag(runtime, error_toasts);
    }
    // Reset transient browser state so a fresh open behaves like a
    // fresh open: empty search, scrolled to the top. We intentionally
    // do NOT auto-focus the search field - the player can click it if
    // they want to type, and most opens are "scroll through and
    // click craft" rather than "type to filter".
    crafting_ui.reset_browser();
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
