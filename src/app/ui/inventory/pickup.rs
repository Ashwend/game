use bevy_egui::egui::{self, pos2};

use crate::{
    app::state::{MenuState, PickupTargetState},
    items::{ItemDefinition, item_definition},
};

use super::super::theme;

pub(super) fn pickup_tooltip(
    ctx: &egui::Context,
    menu: &MenuState,
    pickup_target: &PickupTargetState,
) {
    if menu.pause_open || menu.inventory_open || menu.chat_open {
        return;
    }

    let (Some(screen_position), Some(stack)) =
        (pickup_target.screen_position, pickup_target.stack.as_ref())
    else {
        return;
    };
    let title = item_definition(&stack.item_id)
        .map(|definition: &ItemDefinition| definition.name)
        .unwrap_or(stack.item_id.as_str());
    let body = if stack.quantity > 1 {
        format!("Press E to pick up\nQuantity: {}", stack.quantity)
    } else {
        "Press E to pick up".to_owned()
    };

    theme::anchored_wow_tooltip(
        ctx,
        "pickup_target_tooltip",
        pos2(screen_position.x, screen_position.y),
        title,
        &body,
    );
}
