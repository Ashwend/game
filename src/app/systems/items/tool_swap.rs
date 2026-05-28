use bevy::prelude::*;

use crate::{
    app::state::{LocalPlayerState, MenuState, Screen, ToolSwapState},
    items::item_definition,
};

/// Watches the player's active actionbar slot and drives the tool-swap
/// entry animation timer. Runs once per frame before the swing input system
/// (so swings are correctly blocked while the new tool is still being
/// lifted into view) and before the held-item visual system (so the entry
/// offset is fresh).
pub(crate) fn update_tool_swap_state_system(
    time: Res<Time>,
    local_player: Res<LocalPlayerState>,
    menu: Res<MenuState>,
    mut swap_state: ResMut<ToolSwapState>,
) {
    if menu.screen != Screen::InGame {
        swap_state.reset();
        return;
    }

    let active = local_player
        .private
        .as_ref()
        .and_then(|private| private.inventory.active_actionbar_stack())
        .and_then(|stack| {
            item_definition(&stack.item_id)
                .filter(|definition| definition.equipable)
                .map(|definition| (stack.item_id.as_ref(), definition.model))
        });
    let active_owned = active.map(|(id, model)| (id.to_owned(), model));
    let active_borrowed = active_owned
        .as_ref()
        .map(|(id, model)| (id.as_str(), *model));
    swap_state.observe(time.delta_secs(), active_borrowed);
}
