mod dialogs;
mod session;
mod table;
#[cfg(test)]
mod tests;

use bevy_egui::egui;

use crate::{
    analytics::Analytics,
    app::state::{ClientRuntime, MenuState, SaveStore, Screen, SteamUser},
    net::ClientNetwork,
};

use super::theme::{self, BOUNDED_PANEL_VERTICAL_PADDING, ButtonKind};
use dialogs::{create_world_dialog_ui, edit_world_dialog_ui, open_create_world_dialog};
use session::poll_singleplayer_start;
pub(super) use session::refresh_worlds;
use table::{available_table_height, draw_world_headers, draw_world_table};

pub(super) fn worlds_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
    network: &ClientNetwork,
    analytics: &Analytics,
) {
    theme::screen_scrim(ctx, "worlds_scrim", 145);
    handle_worlds_escape(ctx, menu);
    if poll_singleplayer_start(menu, runtime) {
        ctx.request_repaint();
    }
    theme::bounded_panel(
        ctx,
        "worlds_panel",
        920.0,
        BOUNDED_PANEL_VERTICAL_PADDING,
        BOUNDED_PANEL_VERTICAL_PADDING,
        |ui| {
            let has_worlds = !menu.worlds.is_empty() || !menu.corrupted_worlds.is_empty();
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
            draw_world_headers(ui);
            // The status line under the table needs room when present; the
            // table itself takes whatever vertical space is left in the
            // bounded panel after the header + status reservation.
            let status_reserve = if menu.status.is_some() { 26.0 } else { 0.0 };
            let table_height = available_table_height(ui, status_reserve);
            draw_world_table(ui, menu, store, user, network, analytics, table_height);

            if let Some(status) = &menu.status {
                ui.add_space(10.0);
                ui.label(theme::status_text(status));
            }
        },
    );
    create_world_dialog_ui(ctx, menu, store, user, analytics);
    edit_world_dialog_ui(ctx, menu, store);
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
