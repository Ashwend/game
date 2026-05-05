use bevy_egui::egui;

use crate::app::state::{ClientRuntime, MenuState, SaveStore, Screen};

use super::{danger_menu_button, menu_button, theme};

pub(super) fn pause_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
) {
    let screen_rect = ctx.content_rect();
    let backdrop_response = egui::Area::new("pause_backdrop".into())
        .order(egui::Order::Middle)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let local_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, screen_rect.size());
            let response = ui.allocate_rect(local_rect, egui::Sense::click());
            ui.painter().rect_filled(
                local_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(1, 3, 7, 190),
            );
            response
        })
        .inner;

    if backdrop_response.clicked() {
        menu.pause_open = false;
    }

    egui::Area::new("pause_menu".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(320.0);
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(272.0);
                ui.vertical_centered(|ui| {
                    ui.label(theme::section("Paused"));
                    ui.add_space(16.0);
                    if menu_button(ui, "Resume").clicked() {
                        menu.pause_open = false;
                    }
                    if danger_menu_button(ui, "Quit").clicked() {
                        runtime.shutdown(&store.0);
                        menu.screen = Screen::MainMenu;
                        menu.pause_open = false;
                    }
                });
            });
        });
}
