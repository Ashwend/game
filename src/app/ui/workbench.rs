//! Workbench upgrade modal.
//!
//! Opens when the server's replication includes `local_player.open_workbench`.
//! Unlike the furnace, the workbench has no item slots: it shows the bench's
//! current tier, a one-sentence blurb for what that tier unlocks, and, when the
//! shared upgrade table lists a next tier, a cost list plus an Upgrade button.
//! Costs never travel on the wire; the client reads them from the compile-time
//! upgrade table ([`crate::items::upgrade_for`]) and checks affordability
//! against its own replicated inventory. The server re-validates everything on
//! `Upgrade`, so the button is only a convenience gate.

use bevy_egui::egui::{self, Align2, Color32, Id, Layout, Order, RichText};

use crate::{
    app::{
        state::{ClientRuntime, ErrorToastSink, LocalPlayerState, MenuState},
        systems::send_workbench_command,
    },
    inventory::count_items_in_inventory,
    items::{DeployableKind, item_definition, upgrade_for},
    protocol::{OpenWorkbenchView, PlayerInventoryState, WorkbenchCommand},
};

use super::{item_icons::texture_for, modal::backdrop_layer, theme};

const PANEL_WIDTH: f32 = 420.0;
const ICON_SIZE: f32 = 22.0;
/// Shortfall red, matched to the crafting cost-row missing colour so a short
/// material reads the same across every crafting surface.
const COST_SHORT_COLOR: Color32 = Color32::from_rgb(224, 96, 96);

/// One-sentence unlock blurb per workbench tier. Tier is 1-indexed; anything
/// beyond the table falls back to a generic line so a future tier never panics.
fn tier_blurb(tier: u8) -> &'static str {
    match tier {
        1 => "Assembles tier-1 goods: iron tools, doors, storage, and the furnace.",
        2 => "Adds the heavier bench with anvil and vise for tier-2 gear.",
        _ => "A crafting station.",
    }
}

pub(super) fn workbench_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    local_player: &LocalPlayerState,
    error_toasts: &mut dyn ErrorToastSink,
) {
    if menu.pause_open {
        return;
    }
    // Source of truth is the replicated `PlayerPrivate.open_workbench`: present
    // when the player has a workbench open, absent otherwise.
    let view: OpenWorkbenchView = match local_player
        .private
        .as_ref()
        .and_then(|private| private.open_workbench)
    {
        Some(view) => view,
        None => return,
    };

    let inventory = local_player.private.as_ref().map(|p| &p.inventory);

    // Scrim. Click outside the panel sends Close to the server.
    let backdrop = backdrop_layer(
        ctx,
        "workbench_backdrop",
        Order::Middle,
        theme::backdrop_color(),
    );
    if backdrop.clicked() {
        send_workbench_command(runtime, error_toasts, WorkbenchCommand::Close);
        return;
    }

    let mut close_requested = false;
    let mut upgrade_requested = false;
    egui::Area::new(Id::new("workbench_panel"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(PANEL_WIDTH);
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(PANEL_WIDTH - 48.0);
                draw_panel(
                    ui,
                    &view,
                    inventory,
                    &mut close_requested,
                    &mut upgrade_requested,
                );
            });
        });

    if upgrade_requested {
        send_workbench_command(
            runtime,
            error_toasts,
            WorkbenchCommand::Upgrade { id: view.id },
        );
    }
    if close_requested {
        send_workbench_command(runtime, error_toasts, WorkbenchCommand::Close);
    }
}

fn draw_panel(
    ui: &mut egui::Ui,
    view: &OpenWorkbenchView,
    inventory: Option<&PlayerInventoryState>,
    close_requested: &mut bool,
    upgrade_requested: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label(theme::section("Workbench"));
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            let close_response =
                theme::compact_button(ui, "Close", theme::ButtonKind::Secondary, 84.0);
            theme::record_click_sound(ui, &close_response);
            if close_response.clicked() {
                *close_requested = true;
            }
        });
    });
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!("Tier {}", view.tier))
            .color(theme::accent())
            .strong(),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(tier_blurb(view.tier))
            .color(theme::muted_text())
            .small(),
    );
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(12.0);

    // The upgrade path (if any) comes straight off the shared upgrade table,
    // keyed by the bench's current kind. `None` means the bench is at its
    // ceiling, show the quiet "fully upgraded" state.
    match upgrade_for(DeployableKind::Workbench { tier: view.tier }) {
        Some(upgrade) => {
            let next_tier = match upgrade.to {
                DeployableKind::Workbench { tier } => tier,
                _ => view.tier.saturating_add(1),
            };
            ui.label(theme::field_label(&format!("Upgrade to Tier {next_tier}")));
            ui.add_space(2.0);
            ui.label(
                RichText::new(tier_blurb(next_tier))
                    .color(theme::muted_text())
                    .small(),
            );
            ui.add_space(10.0);

            let mut affordable = true;
            for input in upgrade.cost {
                let have = inventory
                    .map(|inv| count_items_in_inventory(inv, input.item_id))
                    .unwrap_or(0);
                let need = u32::from(input.quantity);
                if have < need {
                    affordable = false;
                }
                draw_cost_row(ui, input.item_id, have, need);
            }

            ui.add_space(12.0);
            // Enabled only when the player can pay; the server re-checks
            // anyway, so a disabled button is a courtesy, not a guard.
            ui.add_enabled_ui(affordable, |ui| {
                let upgrade_response =
                    theme::compact_button(ui, "Upgrade", theme::ButtonKind::Primary, 160.0);
                theme::record_click_sound(ui, &upgrade_response);
                if upgrade_response.clicked() {
                    *upgrade_requested = true;
                }
            });
            if !affordable {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("You don't have the materials yet.")
                        .color(COST_SHORT_COLOR)
                        .small(),
                );
            }
        }
        None => {
            ui.label(
                RichText::new("Fully upgraded.")
                    .color(theme::muted_text())
                    .italics(),
            );
        }
    }
}

/// One `icon  Name   have/need` row, with the count in red when the player is
/// short of the requirement.
fn draw_cost_row(ui: &mut egui::Ui, item_id: &str, have: u32, need: u32) {
    let name = item_definition(item_id)
        .map(|def| def.name)
        .unwrap_or(item_id);
    ui.horizontal(|ui| {
        if let Some(texture) = texture_for(item_id) {
            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                texture,
                egui::vec2(ICON_SIZE, ICON_SIZE),
            )));
        } else {
            // No icon shipped yet (art pass adds them): reserve the slot so the
            // name column still lines up.
            ui.add_space(ICON_SIZE);
        }
        ui.add_space(6.0);
        ui.label(RichText::new(name).color(theme::text()));
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            let color = if have < need {
                COST_SHORT_COLOR
            } else {
                theme::muted_text()
            };
            ui.label(RichText::new(format!("{have}/{need}")).color(color));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        items::{IRON_BAR_ID, METEORITE_INGOT_ID, SALVAGED_FITTINGS_ID},
        protocol::ItemStack,
        server::PlayerPrivate,
    };

    fn workbench_view(tier: u8) -> OpenWorkbenchView {
        OpenWorkbenchView { id: 1, tier }
    }

    fn local_player(
        open_workbench: Option<OpenWorkbenchView>,
        inventory: PlayerInventoryState,
    ) -> LocalPlayerState {
        LocalPlayerState {
            entity: None,
            private: Some(PlayerPrivate {
                inventory,
                crafting: Default::default(),
                open_furnace: None,
                open_loot_bag: None,
                open_workbench,
                last_processed_input: 0,
                applied_action_seq: 0,
                run_speed_multiplier: 1.0,
            }),
            lifecycle: None,
        }
    }

    fn affordable_inventory() -> PlayerInventoryState {
        let mut inventory = PlayerInventoryState::empty();
        inventory.inventory_slots[0] = Some(ItemStack::new(IRON_BAR_ID, 30));
        inventory.inventory_slots[1] = Some(ItemStack::new(SALVAGED_FITTINGS_ID, 6));
        inventory.inventory_slots[2] = Some(ItemStack::new(METEORITE_INGOT_ID, 4));
        inventory
    }

    fn run_ui(f: impl FnMut(&egui::Context)) -> egui::FullOutput {
        let ctx = egui::Context::default();
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 768.0),
                )),
                ..Default::default()
            },
            f,
        )
    }

    #[test]
    fn workbench_ui_noop_without_open_workbench() {
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();
        let local = local_player(None, PlayerInventoryState::empty());
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            workbench_ui(ctx, &mut menu, &mut runtime, &local, &mut toasts);
        });
        assert!(output.shapes.is_empty());
    }

    #[test]
    fn workbench_ui_suppressed_while_paused() {
        let mut menu = MenuState {
            pause_open: true,
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let local = local_player(Some(workbench_view(1)), affordable_inventory());
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            workbench_ui(ctx, &mut menu, &mut runtime, &local, &mut toasts);
        });
        assert!(output.shapes.is_empty());
    }

    #[test]
    fn workbench_ui_renders_upgrade_panel_at_tier_one() {
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();
        let local = local_player(Some(workbench_view(1)), affordable_inventory());
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            workbench_ui(ctx, &mut menu, &mut runtime, &local, &mut toasts);
        });
        // A tier-1 workbench has an upgrade row, so the panel paints.
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn workbench_ui_renders_fully_upgraded_at_top_tier() {
        // A tier-2 workbench has no table row, so the quiet "fully upgraded"
        // state renders instead of an upgrade section. Still paints a panel.
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();
        let local = local_player(Some(workbench_view(2)), PlayerInventoryState::empty());
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            workbench_ui(ctx, &mut menu, &mut runtime, &local, &mut toasts);
        });
        assert!(!output.shapes.is_empty());
    }
}
