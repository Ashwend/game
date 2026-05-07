use bevy_egui::egui;

use crate::{
    app::state::{ClientRuntime, MenuState, Screen, SteamUser},
    steam::{OfflineSteamBackend, SteamBackend},
};

use super::theme::{self, ButtonKind};

pub(super) fn multiplayer_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    _runtime: &mut ClientRuntime,
    _user: &SteamUser,
) {
    theme::screen_scrim(ctx, "multiplayer_scrim", 145);
    theme::anchored_panel(
        ctx,
        "multiplayer_panel",
        560.0,
        egui::Align2::CENTER_CENTER,
        [0.0, -10.0],
        |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::section("Multiplayer"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::compact_button(ui, "Back", ButtonKind::Secondary, 78.0).clicked() {
                        menu.screen = Screen::MainMenu;
                    }
                });
            });

            ui.add_space(16.0);
            theme::inset_frame().show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(theme::field_label("Steam"));
                    if theme::game_button(ui, "Open Server Browser", ButtonKind::Primary, 300.0)
                        .clicked()
                    {
                        let steam = OfflineSteamBackend;
                        menu.status = match steam.open_server_browser() {
                            Ok(()) => Some("opened Steam server browser".to_owned()),
                            Err(error) => Some(format!("Steam browser unavailable: {error}")),
                        };
                    }
                });
            });

            ui.add_space(12.0);
            theme::inset_frame().show(ui, |ui| {
                ui.label(theme::field_label("Direct UDP"));
                ui.add_space(4.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 38.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_sized([320.0, 34.0], theme::text_input(&mut menu.multiplayer_addr));
                        if theme::compact_button(ui, "Connect", ButtonKind::Primary, 92.0).clicked()
                        {
                            menu.status =
                                Some("client networking is moving to Lightyear replication".into());
                        }
                    },
                );
            });

            if let Some(status) = &menu.status {
                ui.add_space(10.0);
                ui.label(theme::status_text(status));
            }
        },
    );
}
