use std::net::SocketAddr;

use bevy_egui::egui;

use crate::{
    app::state::{ClientRuntime, MenuState, Screen, SteamUser},
    net::ClientSession,
    steam::{OfflineSteamBackend, SteamBackend},
};

use super::theme::{self, ButtonKind};

pub(super) fn multiplayer_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    user: &SteamUser,
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
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [320.0, 34.0],
                        egui::TextEdit::singleline(&mut menu.multiplayer_addr),
                    );
                    if theme::compact_button(ui, "Connect", ButtonKind::Primary, 92.0).clicked() {
                        match menu.multiplayer_addr.parse::<SocketAddr>() {
                            Ok(addr) => match ClientSession::connect_udp(addr, &user.0) {
                                Ok(session) => {
                                    runtime.start_session(session, None);
                                    menu.screen = Screen::InGame;
                                    menu.pause_open = false;
                                    menu.status = None;
                                }
                                Err(error) => {
                                    menu.status = Some(format!("connect failed: {error}"));
                                }
                            },
                            Err(error) => menu.status = Some(format!("invalid address: {error}")),
                        }
                    }
                });
            });

            if let Some(status) = &menu.status {
                ui.add_space(10.0);
                ui.label(theme::status_text(status));
            }
        },
    );
}
