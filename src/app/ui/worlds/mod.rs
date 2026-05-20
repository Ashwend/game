mod dialogs;
mod session;
mod table;
#[cfg(test)]
mod tests;

use bevy_egui::egui;

use crate::{
    app::state::{ClientRuntime, MenuState, SaveStore, Screen, SteamUser},
    save::CorruptedWorld,
};

use super::theme::{self, ButtonKind};
use dialogs::{create_world_dialog_ui, edit_world_dialog_ui, open_create_world_dialog};
use session::poll_singleplayer_start;
pub(super) use session::refresh_worlds;
use table::{draw_world_headers, draw_world_table, table_height};

pub(super) const BUTTON_HEIGHT: f32 = 34.0;

pub(super) fn worlds_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
) {
    theme::screen_scrim(ctx, "worlds_scrim", 145);
    handle_worlds_escape(ctx, menu);
    if poll_singleplayer_start(menu, runtime) {
        ctx.request_repaint();
    }
    theme::anchored_panel(
        ctx,
        "worlds_panel",
        920.0,
        egui::Align2::CENTER_CENTER,
        [0.0, -8.0],
        |ui| {
            let has_worlds = !menu.worlds.is_empty();
            let starting_world = menu.world_start.is_some();
            ui.horizontal(|ui| {
                ui.label(theme::section("Singleplayer Worlds"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(!starting_world, |ui| {
                        if theme::compact_button(ui, "Back", ButtonKind::Secondary, 78.0).clicked()
                        {
                            menu.screen = Screen::MainMenu;
                        }
                    });
                    if has_worlds
                        && !starting_world
                        && theme::compact_button(ui, "Create New World", ButtonKind::Primary, 142.0)
                            .clicked()
                    {
                        open_create_world_dialog(menu);
                    }
                });
            });

            ui.add_space(16.0);
            draw_corrupted_worlds_banner(ui, &menu.corrupted_worlds);
            draw_world_headers(ui);
            let table_height = table_height(ctx);
            draw_world_table(ui, menu, store, user, table_height);

            if let Some(status) = &menu.status {
                ui.add_space(10.0);
                ui.label(theme::status_text(status));
            }
        },
    );
    create_world_dialog_ui(ctx, menu, store, user);
    edit_world_dialog_ui(ctx, menu, store);
}

/// Renders a warning banner above the worlds table for save files that
/// could not be loaded. Each entry shows the file name and a tooltip with
/// the underlying parse error so the player has enough info to either
/// delete or recover the file.
fn draw_corrupted_worlds_banner(ui: &mut egui::Ui, corrupted: &[CorruptedWorld]) {
    if corrupted.is_empty() {
        return;
    }

    let fill = egui::Color32::from_rgba_unmultiplied(58, 24, 16, 220);
    let stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgba_unmultiplied(220, 120, 80, 200),
    );
    egui::Frame::NONE
        .fill(fill)
        .stroke(stroke)
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let heading = if corrupted.len() == 1 {
                "1 save couldn't be loaded".to_owned()
            } else {
                format!("{} saves couldn't be loaded", corrupted.len())
            };
            ui.label(
                egui::RichText::new(heading)
                    .size(13.5)
                    .strong()
                    .color(egui::Color32::from_rgb(252, 224, 196)),
            );
            ui.add_space(4.0);
            for entry in corrupted {
                let line = ui.label(
                    egui::RichText::new(format!("• {}", entry.file_name))
                        .size(12.5)
                        .color(egui::Color32::from_rgb(244, 210, 192)),
                );
                let _ = theme::wow_tooltip(line, &entry.file_name, &entry.error);
            }
        });
    ui.add_space(10.0);
}

fn handle_worlds_escape(ctx: &egui::Context, menu: &mut MenuState) {
    if !ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        return;
    }

    if menu.world_start.is_some() {
        ctx.request_repaint();
        return;
    }

    if let Some(dialog) = menu.create_world.as_mut() {
        dialog.closing = true;
        dialog.confirmed = false;
        ctx.request_repaint();
        return;
    }

    if let Some(dialog) = menu.edit_world.as_mut() {
        dialog.closing = true;
        dialog.confirmed = false;
        ctx.request_repaint();
        return;
    }

    if let Some(dialog) = menu.confirmation.as_mut() {
        dialog.closing = true;
        dialog.confirmed = false;
        ctx.request_repaint();
        return;
    }

    menu.screen = Screen::MainMenu;
}
