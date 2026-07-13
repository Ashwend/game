use bevy_egui::egui::{self, pos2};

use crate::{
    app::state::{CupboardAuthState, MenuState, PickupTargetState},
    items::{DeployableKind, ItemDefinition, ToolKind, item_definition},
    resources::resource_node_definition,
};

use super::super::theme;

pub(in crate::app::ui) fn pickup_tooltip(
    ctx: &egui::Context,
    menu: &MenuState,
    pickup_target: &PickupTargetState,
    hammer_equipped: bool,
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

    let Some((title, body)) = pickup_tooltip_text(pickup_target, hammer_equipped) else {
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

fn pickup_tooltip_text(
    pickup_target: &PickupTargetState,
    hammer_equipped: bool,
) -> Option<(String, String)> {
    if let Some(stack) = pickup_target.stack.as_ref() {
        let title = item_definition(&stack.item_id)
            .map(|definition: &ItemDefinition| definition.name)
            .unwrap_or(stack.item_id.as_ref())
            .to_owned();
        // The dropped stack's quantity is deliberately left off: it's clutter on
        // a "press E" affordance, and the count is shown again in the inventory
        // once the item is picked up.
        return Some((title, "Press E to pick up".to_owned()));
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
        // Building blocks only matter when the hammer is in hand (their
        // tooltip is the repair/upgrade/stability readout); without it
        // the label is noise over every wall the player walks past.
        if matches!(kind, DeployableKind::Building { .. }) && !hammer_equipped {
            return None;
        }
        return Some(deployable_tooltip_text(
            kind,
            pickup_target.deployable_stability,
            pickup_target.deployable_cupboard_auth,
        ));
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
    // No remaining-yield readout. How much a node (tree, ore vein, crude pickup)
    // has left is communicated by its visual depletion state, so the tooltip
    // stays a "what is this / how do I gather it" hint and the node running dry
    // stays a surprise.
    //
    // Crude nodes (branches, surface stones, grass tufts) are quick-grab only,
    // swinging at them does nothing, so the tooltip only mentions E.
    let body = if definition.required_tool.kind == ToolKind::Hands {
        "Press E to pick up".to_owned()
    } else {
        format!(
            "Hold Left Mouse to gather\nRequires: {}",
            definition.required_tool.label()
        )
    };
    Some((definition.name.to_owned(), body))
}

fn deployable_tooltip_text(
    kind: DeployableKind,
    stability: Option<u8>,
    cupboard_auth: Option<CupboardAuthState>,
) -> (String, String) {
    match kind {
        DeployableKind::Furnace { tier } => (
            format!("Furnace T{tier}"),
            "Press E to open\nLoad fuel + smeltable ore".to_owned(),
        ),
        DeployableKind::Workbench { tier } => (
            format!("Workbench lvl {tier}"),
            "Crafting station. Tier-1 recipes unlock while you're in range.".to_owned(),
        ),
        DeployableKind::Building { piece, tier } => {
            let stability_line = stability
                .map(|pct| format!("Stability: {pct}%\n"))
                .unwrap_or_default();
            (
                format!("{} ({})", piece.label(), tier.label()),
                format!(
                    "{stability_line}Repair with a hammer swing\nHold right click with a hammer for options"
                ),
            )
        }
        DeployableKind::Door { variant } => (
            variant.label().to_owned(),
            "Press E to open or close\nHold E to pick up\nHold right click to change the code"
                .to_owned(),
        ),
        DeployableKind::SleepingBag => (
            "Sleeping Bag".to_owned(),
            "Press E to pick up\nHold E to rename".to_owned(),
        ),
        DeployableKind::StorageBox { .. } => {
            (kind.label().to_owned(), "Press E to open".to_owned())
        }
        DeployableKind::RuinCache => (
            "Salvage Chest".to_owned(),
            "Press E to loot\nSalvaged fittings and supplies. Restocks over time.".to_owned(),
        ),
        DeployableKind::Torch { .. } => (
            "Torch".to_owned(),
            "Burns for hours to light the area.".to_owned(),
        ),
        DeployableKind::ToolCupboard => {
            let body = match cupboard_auth {
                Some(CupboardAuthState::Authorized) => {
                    "You are authorized here\nPress E to remove yourself\nHold E for options"
                }
                Some(CupboardAuthState::Unauthorized) | None => {
                    "You are not authorized here\nPress E to authorize yourself\nHold E for options"
                }
            };
            ("Tool Cupboard".to_owned(), body.to_owned())
        }
        DeployableKind::Explosive { kind } => (
            kind.label().to_owned(),
            "Armed and ticking\nHold E to defuse it (recover half the materials)\nOr hit it to fizzle it, no refund".to_owned(),
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
            pickup_tooltip(ctx, &menu, &pickup_target, false);
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
                pickup_tooltip(ctx, &menu, &pickup_target, false);
            });

            assert!(
                output.shapes.is_empty(),
                "tooltip should be hidden for {label}",
            );
        }
    }

    #[test]
    fn building_tooltip_only_shows_with_the_hammer() {
        let target = PickupTargetState {
            deployable_id: Some(1),
            deployable_kind: Some(DeployableKind::Building {
                piece: crate::building::BuildingPiece::Wall,
                tier: crate::building::BuildingTier::Sticks,
            }),
            deployable_stability: Some(90),
            screen_position: Some(Vec2::new(100.0, 120.0)),
            ..Default::default()
        };
        assert!(
            pickup_tooltip_text(&target, false).is_none(),
            "building blocks stay quiet without the hammer"
        );
        let (_, body) = pickup_tooltip_text(&target, true).expect("hammer shows the readout");
        assert!(body.contains("Stability: 90%"));

        // Non-building deployables keep their tooltip regardless.
        let bag = PickupTargetState {
            deployable_id: Some(2),
            deployable_kind: Some(DeployableKind::SleepingBag),
            screen_position: Some(Vec2::new(100.0, 120.0)),
            ..Default::default()
        };
        assert!(pickup_tooltip_text(&bag, false).is_some());
    }

    #[test]
    fn resource_tooltip_shows_requirement_but_not_remaining_yield() {
        let pickup_target = PickupTargetState {
            resource_node_id: Some(1),
            resource_definition_id: Some(COAL_NODE_ID.to_owned()),
            resource_storage: vec![ItemStack::new("coal", 6)],
            screen_position: Some(Vec2::new(100.0, 120.0)),
            ..Default::default()
        };

        let (_, body) = pickup_tooltip_text(&pickup_target, false).expect("resource tooltip");

        // Any pickaxe mines a tier-1 node, so the requirement names just the
        // tool kind. "tier 1" in the tooltip implied a tool level that
        // doesn't exist for the player.
        assert!(body.contains("Requires: Pickaxe"));
        assert!(!body.contains("tier"));
        // How much the node has left is conveyed by its visual depletion, never
        // the tooltip, so the remaining yield must not leak here.
        assert!(!body.contains("Contents"));
        assert!(!body.contains("Coal"));
    }
}
