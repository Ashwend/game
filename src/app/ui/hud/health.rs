//! The bottom-right health bar: icon cell, numeric readout, and fill fraction
//! clamped against `MAX_HEALTH`. The root `hud_ui` anchors it inside the
//! `hud_bars` Area.

use bevy_egui::egui;

use crate::protocol::MAX_HEALTH;

const HEALTH_WIDTH: f32 = 192.0;
const HEALTH_HEIGHT: f32 = 30.0;
const HEALTH_ICON_WIDTH: f32 = 30.0;

pub(super) fn health_bar(ui: &mut egui::Ui, health: f32) {
    let fraction = (health / MAX_HEALTH).clamp(0.0, 1.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::new(HEALTH_WIDTH, HEALTH_HEIGHT),
        egui::Sense::hover(),
    );
    let icon_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.left() + HEALTH_ICON_WIDTH, rect.bottom()),
    );
    let bar_rect = egui::Rect::from_min_max(
        egui::pos2(icon_rect.right(), rect.top()),
        rect.right_bottom(),
    );
    let fill_rect = egui::Rect::from_min_max(
        bar_rect.min,
        egui::pos2(
            bar_rect.left() + bar_rect.width() * fraction,
            bar_rect.bottom(),
        ),
    );

    ui.painter().rect_filled(
        rect,
        1,
        egui::Color32::from_rgba_unmultiplied(30, 29, 24, 202),
    );
    ui.painter().rect_filled(
        icon_rect,
        1,
        egui::Color32::from_rgba_unmultiplied(50, 48, 42, 226),
    );
    ui.painter()
        .rect_filled(fill_rect, 0, egui::Color32::from_rgb(125, 196, 55));
    ui.painter().rect_stroke(
        rect,
        1,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 28),
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        icon_rect.center(),
        egui::Align2::CENTER_CENTER,
        "+",
        egui::FontId::monospace(22.0),
        egui::Color32::from_rgb(222, 229, 215),
    );
    ui.painter().text(
        egui::pos2(bar_rect.left() + 10.0, bar_rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{health:.0}"),
        egui::FontId::monospace(16.0),
        egui::Color32::from_rgb(240, 247, 232),
    );
}

#[cfg(test)]
mod tests {
    use super::super::raw_input;
    use super::*;

    #[test]
    fn health_bar_clamps_extreme_values() {
        let ctx = egui::Context::default();

        let _ = ctx.run_ui(raw_input(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                health_bar(ui, -10.0);
                health_bar(ui, MAX_HEALTH * 2.0);
            });
        });
    }

    #[test]
    fn health_bar_renders_full_mid_and_empty() {
        // Every fill fraction runs the painter without panicking and emits
        // the frame + icon + fill + text shapes.
        for health in [0.0, MAX_HEALTH * 0.5, MAX_HEALTH] {
            let ctx = egui::Context::default();
            let output = ctx.run_ui(raw_input(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    health_bar(ui, health);
                });
            });
            assert!(!output.shapes.is_empty());
        }
    }
}
