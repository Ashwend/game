use bevy_egui::egui;

use crate::{
    app::state::ClientRuntime,
    protocol::{MAX_HEALTH, MAX_STAMINA},
};

use super::theme;

const HUD_WIDTH: f32 = 255.0;
const BAR_WIDTH: f32 = 215.0;
const BAR_HEIGHT: f32 = 13.0;

pub(super) fn hud_ui(ctx: &egui::Context, runtime: &ClientRuntime) {
    let Some(player) = runtime.local_view() else {
        return;
    };

    egui::Area::new("hud_bars".into())
        .anchor(egui::Align2::LEFT_TOP, [16.0, 16.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(6, 9, 13, 174))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(115, 132, 151, 72),
                ))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(HUD_WIDTH);
                    status_bar(
                        ui,
                        "Health",
                        player.health,
                        MAX_HEALTH,
                        egui::Color32::from_rgb(190, 55, 58),
                    );
                    ui.add_space(6.0);
                    status_bar(
                        ui,
                        "Stamina",
                        player.stamina,
                        MAX_STAMINA,
                        egui::Color32::from_rgb(61, 159, 104),
                    );
                });
        });
}

fn status_bar(ui: &mut egui::Ui, label: &str, value: f32, max: f32, color: egui::Color32) {
    let fraction = (value / max).clamp(0.0, 1.0);
    ui.horizontal(|ui| {
        ui.label(theme::field_label(label));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{value:.0}/{max:.0}"))
                    .monospace()
                    .size(12.0)
                    .color(theme::muted_text()),
            );
        });
    });

    let (rect, _) =
        ui.allocate_exact_size(egui::Vec2::new(BAR_WIDTH, BAR_HEIGHT), egui::Sense::hover());
    let fill_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.min.x + rect.width() * fraction, rect.max.y),
    );
    ui.painter()
        .rect_filled(rect, 4, egui::Color32::from_rgba_unmultiplied(4, 6, 9, 218));
    ui.painter().rect_filled(fill_rect, 4, color);
    ui.painter().rect_stroke(
        rect,
        4,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 34),
        ),
        egui::StrokeKind::Inside,
    );
}
