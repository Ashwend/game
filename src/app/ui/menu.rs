use bevy::{app::AppExit, prelude::*};
use bevy_egui::egui;

use crate::{
    app::state::{MenuState, SaveStore, Screen, SteamUser},
    steam::{OfflineSteamBackend, SteamBackend},
};

use super::{
    danger_menu_button, menu_button, primary_menu_button,
    theme::{self, MENU_WIDTH},
    worlds::refresh_worlds,
};

pub(super) fn main_menu_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    store: &SaveStore,
    user: &SteamUser,
    app_exit: &mut MessageWriter<AppExit>,
) {
    theme::screen_scrim(ctx, "main_menu_scrim", 150);
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
                        if menu_button(ui, "Multiplayer").clicked() {
                            let steam = OfflineSteamBackend;
                            menu.status = match steam.open_server_browser() {
                                Ok(()) => Some("opened Steam server browser".to_owned()),
                                Err(error) => Some(format!("Steam browser unavailable: {error}")),
                            };
                            menu.screen = Screen::Multiplayer;
                        }
                        if danger_menu_button(ui, "Quit").clicked() {
                            app_exit.write(AppExit::Success);
                        }
                    });
                });

                ui.add_space(14.0);
                ui.label(theme::muted(format!(
                    "Signed in as {}",
                    user.0.display_name
                )));
                if let Some(status) = &menu.status {
                    ui.add_space(4.0);
                    ui.label(theme::status_text(status));
                }
            });
        });
}
