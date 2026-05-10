use bevy_egui::egui;

use crate::{
    app::state::{MenuState, SaveStore, Screen, SteamUser},
    protocol::GAME_VERSION,
};

use super::{
    danger_menu_button, primary_menu_button,
    theme::{self, MENU_WIDTH},
    worlds::refresh_worlds,
};

pub(super) fn main_menu_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    store: &SaveStore,
    user: &SteamUser,
) {
    theme::screen_scrim(ctx, "main_menu_scrim", 118);
    egui::Area::new("main_menu".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -20.0])
        .show(ctx, |ui| {
            ui.set_width(MENU_WIDTH);
            ui.vertical_centered(|ui| {
                ui.label(theme::title("Game", 78.0));
                ui.add_space(20.0);
                theme::panel_frame().show(ui, |ui| {
                    ui.set_width(MENU_WIDTH - 48.0);
                    ui.vertical_centered(|ui| {
                        if primary_menu_button(ui, "Singleplayer").clicked() {
                            refresh_worlds(menu, store);
                            menu.screen = Screen::Worlds;
                        }
                        if primary_menu_button(ui, "Multiplayer").clicked() {
                            menu.screen = Screen::Multiplayer;
                        }
                        if super::menu_button(ui, "Options").clicked() {
                            menu.screen = Screen::Options;
                        }
                        if danger_menu_button(ui, "Quit").clicked() {
                            menu.quit_requested = true;
                        }
                    });
                });

                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new(format!("Signed in as {}", user.0.display_name))
                        .color(theme::text()),
                );
                if let Some(status) = &menu.status {
                    ui.add_space(4.0);
                    ui.label(theme::status_text(status));
                }
            });
        });
    draw_version_indicator(ctx);
}

fn draw_version_indicator(ctx: &egui::Context) {
    egui::Area::new("main_menu_version".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, [-18.0, -14.0])
        .show(ctx, |ui| {
            ui.label(theme::muted(format!("v{GAME_VERSION}")));
        });
}
