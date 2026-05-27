use bevy_egui::egui;

use crate::{
    analytics::Analytics,
    app::state::{ClientRuntime, DirectConnectDialog, MenuState, Screen, SteamUser},
    net::ClientNetwork,
    steam::{OfflineSteamBackend, SteamBackend},
};

mod direct_connect;

use super::theme::{self, BOUNDED_PANEL_VERTICAL_PADDING, BoundedPanelFill, ButtonKind};
use direct_connect::direct_connect_dialog_ui;

pub(super) fn multiplayer_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    user: &SteamUser,
    network: &ClientNetwork,
    analytics: &Analytics,
) {
    theme::screen_scrim(ctx, "multiplayer_scrim", 145);
    handle_multiplayer_escape(ctx, menu);
    theme::bounded_panel(
        ctx,
        "multiplayer_panel",
        560.0,
        BOUNDED_PANEL_VERTICAL_PADDING,
        BOUNDED_PANEL_VERTICAL_PADDING,
        BoundedPanelFill::ToContent,
        |ui| {
            let body_height = ui.available_height();
            egui::ScrollArea::vertical()
                .id_salt("multiplayer_scroll")
                .max_height(body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    draw_multiplayer_header(ui, menu);

                    ui.add_space(16.0);
                    theme::inset_frame().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.vertical(|ui| {
                            ui.label(theme::field_label("Steam"));
                            if theme::game_button(
                                ui,
                                "Open Server Browser",
                                ButtonKind::Primary,
                                ui.available_width(),
                            )
                            .clicked()
                            {
                                let steam = OfflineSteamBackend;
                                menu.status = match steam.open_server_browser() {
                                    Ok(()) => Some("opened Steam server browser".to_owned()),
                                    Err(error) => {
                                        Some(format!("Steam browser unavailable: {error}"))
                                    }
                                };
                            }
                        });
                    });

                    if let Some(status) = &menu.status {
                        ui.add_space(10.0);
                        ui.label(theme::status_text(status));
                    }
                });
        },
    );
    direct_connect_dialog_ui(ctx, menu, runtime, user, network, analytics);
}

fn draw_multiplayer_header(ui: &mut egui::Ui, menu: &mut MenuState) {
    if ui.available_width() < 340.0 {
        ui.label(theme::section("Multiplayer"));
        ui.add_space(4.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            draw_header_buttons(ui, menu);
        });
        return;
    }

    ui.horizontal(|ui| {
        ui.label(theme::section("Multiplayer"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            draw_header_buttons(ui, menu);
        });
    });
}

fn draw_header_buttons(ui: &mut egui::Ui, menu: &mut MenuState) {
    if theme::compact_button(ui, "Back", ButtonKind::Secondary, 78.0).clicked() {
        menu.screen = Screen::MainMenu;
    }
    if theme::compact_button(ui, "Direct Connect", ButtonKind::Primary, 128.0).clicked() {
        menu.direct_connect = Some(DirectConnectDialog::new(&menu.multiplayer_addr));
    }
}

fn handle_multiplayer_escape(ctx: &egui::Context, menu: &mut MenuState) {
    if !ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        return;
    }

    if let Some(dialog) = menu.direct_connect.as_mut() {
        if dialog.is_connecting() {
            ctx.request_repaint();
            return;
        }

        dialog.closing = true;
        ctx.request_repaint();
        return;
    }

    menu.screen = Screen::MainMenu;
}
