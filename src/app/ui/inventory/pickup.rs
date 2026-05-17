use bevy_egui::egui::{self, pos2};

use crate::{
    app::state::{MenuState, PickupTargetState},
    items::{ItemDefinition, item_definition},
    resources::resource_node_definition,
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

    let Some(screen_position) = pickup_target.screen_position else {
        return;
    };

    let Some((title, body)) = pickup_tooltip_text(pickup_target) else {
        return;
    };

    theme::anchored_wow_tooltip(
        ctx,
        "pickup_target_tooltip",
        pos2(screen_position.x, screen_position.y),
        &title,
        &body,
    );
}

fn pickup_tooltip_text(pickup_target: &PickupTargetState) -> Option<(String, String)> {
    if let Some(stack) = pickup_target.stack.as_ref() {
        let title = item_definition(&stack.item_id)
            .map(|definition: &ItemDefinition| definition.name)
            .unwrap_or(stack.item_id.as_str())
            .to_owned();
        let body = if stack.quantity > 1 {
            format!("Press E to pick up\nQuantity: {}", stack.quantity)
        } else {
            "Press E to pick up".to_owned()
        };
        return Some((title, body));
    }

    let definition_id = pickup_target.resource_definition_id.as_ref()?;
    let definition = resource_node_definition(definition_id)?;
    let contents = if pickup_target.resource_storage.is_empty() {
        "Empty".to_owned()
    } else {
        pickup_target
            .resource_storage
            .iter()
            .map(|stack| {
                let name = item_definition(&stack.item_id)
                    .map(|definition| definition.name)
                    .unwrap_or(stack.item_id.as_str());
                format!("{name}: {}", stack.quantity)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let body = format!(
        "Hold Left Mouse to gather\nRequires: {}\nContents:\n{}",
        definition.required_tool.label(),
        contents
    );
    Some((definition.name.to_owned(), body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        protocol::{ItemStack, Vec3Net},
        resources::COAL_NODE_ID,
    };
    use bevy::prelude::Vec2;

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            ..Default::default()
        }
    }

    #[test]
    fn pickup_tooltip_renders_available_target() {
        let ctx = egui::Context::default();
        let menu = MenuState::default();
        let pickup_target = PickupTargetState {
            dropped_item_id: Some(1),
            stack: Some(ItemStack::new("unknown-item", 3)),
            world_position: Some(Vec3Net::new(1.0, 2.0, 3.0)),
            screen_position: Some(Vec2::new(100.0, 120.0)),
            ..Default::default()
        };

        let output = ctx.run(raw_input(), |ctx| {
            pickup_tooltip(ctx, &menu, &pickup_target);
        });

        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn pickup_tooltip_is_hidden_when_ui_blocks_pickup() {
        let ctx = egui::Context::default();
        let menu = MenuState {
            inventory_open: true,
            ..Default::default()
        };
        let pickup_target = PickupTargetState {
            stack: Some(ItemStack::new("unknown-item", 1)),
            screen_position: Some(Vec2::new(100.0, 120.0)),
            ..Default::default()
        };

        let output = ctx.run(raw_input(), |ctx| {
            pickup_tooltip(ctx, &menu, &pickup_target);
        });

        assert!(output.shapes.is_empty());
    }

    #[test]
    fn resource_tooltip_lists_requirement_and_contents() {
        let pickup_target = PickupTargetState {
            resource_node_id: Some(1),
            resource_definition_id: Some(COAL_NODE_ID.to_owned()),
            resource_storage: vec![ItemStack::new("coal", 6)],
            screen_position: Some(Vec2::new(100.0, 120.0)),
            ..Default::default()
        };

        let (_, body) = pickup_tooltip_text(&pickup_target).expect("resource tooltip");

        assert!(body.contains("Pickaxe tier 1"));
        assert!(body.contains("Coal: 6"));
    }
}
