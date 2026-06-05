use bevy_egui::egui::{self, pos2};

use crate::{
    app::state::{MenuState, PickupTargetState},
    items::{DeployableKind, ItemDefinition, ToolKind, item_definition},
    resources::resource_node_definition,
};

use super::super::theme;

pub(in crate::app::ui) fn pickup_tooltip(
    ctx: &egui::Context,
    menu: &MenuState,
    pickup_target: &PickupTargetState,
) {
    // Any full-screen modal hides the world tooltip. Even though the
    // scrim is opaque on top, egui's foreground-area tooltip would
    // still paint over it, the modal also blocks the input that would
    // act on the target, so the tooltip would be lying about what
    // pressing E does.
    if menu.pause_open
        || menu.inventory_open
        || menu.crafting_open
        || menu.furnace_open
        || menu.loot_bag_open
        || menu.chat_open
        || menu.death_splash.is_some()
    {
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
            .unwrap_or(stack.item_id.as_ref())
            .to_owned();
        let body = if stack.quantity > 1 {
            format!("Press E to pick up\nQuantity: {}", stack.quantity)
        } else {
            "Press E to pick up".to_owned()
        };
        return Some((title, body));
    }

    // A logged-out sleeping body identifies itself: who it is and how much
    // health it has left, so a passer-by can decide whether to execute or
    // rob it. (E-to-loot is wired separately; until then the player can still
    // swing on the body to kill it and loot the dropped bag.)
    if let Some((name, health)) = pickup_target.sleeping_player.as_ref() {
        return Some((
            name.clone(),
            format!(
                "Sleeping\n{} HP\nPress E to loot",
                health.round().max(0.0) as i32
            ),
        ));
    }

    // Placed structures fall through next so the player can see the
    // "Press E to open" affordance the same way they see "Press E to
    // pick up" on dropped items. Workbenches have no interactive view
    // yet, they show a passive "in range" status instead so the
    // player understands what the structure does without us inventing
    // an interaction that doesn't exist.
    if let Some(kind) = pickup_target.deployable_kind {
        return Some(deployable_tooltip_text(kind));
    }

    // Loot bags use the same "Press E to open" wording as a furnace;
    // the bag itself is a container so the interaction shape lines
    // up. Item count would be ideal here but the pickup target
    // doesn't carry the bag's contents, the bag is opened
    // server-side and contents arrive via `PlayerPrivate.open_loot_bag`.
    if pickup_target.loot_bag_id.is_some() {
        return Some((
            "Loot bag".to_owned(),
            "Press E to open\nDrag items between the bag and your inventory.".to_owned(),
        ));
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
                    .unwrap_or(stack.item_id.as_ref());
                format!("{name}: {}", stack.quantity)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    // Crude nodes (branches, surface stones, grass tufts) are quick-grab
    // only, swinging at them does nothing, so the tooltip only mentions E.
    let action_line = if definition.required_tool.kind == ToolKind::Hands {
        "Press E to pick up".to_owned()
    } else {
        format!(
            "Hold Left Mouse to gather\nRequires: {}",
            definition.required_tool.label()
        )
    };
    let body = format!("{action_line}\nContents:\n{contents}");
    Some((definition.name.to_owned(), body))
}

fn deployable_tooltip_text(kind: DeployableKind) -> (String, String) {
    match kind {
        DeployableKind::Furnace { tier } => (
            format!("Furnace T{tier}"),
            "Press E to open\nLoad fuel + smeltable ore".to_owned(),
        ),
        DeployableKind::Workbench { tier } => (
            format!("Workbench lvl {tier}"),
            "Crafting station. Tier-1 recipes unlock while you're in range.".to_owned(),
        ),
    }
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
        // Every modal that gates pickup input should also hide the
        // tooltip, otherwise the world-anchored label leaks through
        // the scrim and tells the player to press a key that no
        // longer does anything.
        type SetFlag = fn(&mut MenuState);
        let cases: &[(&str, SetFlag)] = &[
            ("inventory_open", |m| m.inventory_open = true),
            ("crafting_open", |m| m.crafting_open = true),
            ("furnace_open", |m| m.furnace_open = true),
            ("chat_open", |m| m.chat_open = true),
            ("pause_open", |m| m.pause_open = true),
        ];
        for (label, set_flag) in cases {
            let mut menu = MenuState::default();
            set_flag(&mut menu);

            let ctx = egui::Context::default();
            let pickup_target = PickupTargetState {
                stack: Some(ItemStack::new("unknown-item", 1)),
                screen_position: Some(Vec2::new(100.0, 120.0)),
                ..Default::default()
            };

            let output = ctx.run(raw_input(), |ctx| {
                pickup_tooltip(ctx, &menu, &pickup_target);
            });

            assert!(
                output.shapes.is_empty(),
                "tooltip should be hidden for {label}",
            );
        }
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
