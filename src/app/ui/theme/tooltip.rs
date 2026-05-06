use bevy_egui::egui::{self, Color32, Frame, Margin, RichText, Stroke, vec2};

use super::text;

pub(in crate::app::ui) fn wow_tooltip(
    response: egui::Response,
    title: &str,
    body: &str,
) -> egui::Response {
    if let Some(pointer_position) = response.hover_pos().or_else(|| {
        response
            .contains_pointer()
            .then(|| response.ctx.pointer_hover_pos())
            .flatten()
    }) {
        let tooltip_position = pointer_position + vec2(16.0, 18.0);
        egui::Area::new(response.id.with("wow_tooltip"))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .fixed_pos(tooltip_position)
            .show(&response.ctx, |ui| {
                draw_wow_tooltip(ui, title, body);
            });
    }

    response
}

fn draw_wow_tooltip(ui: &mut egui::Ui, title: &str, body: &str) {
    Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(4, 6, 12, 244))
        .stroke(Stroke::new(1.0, Color32::from_rgb(78, 112, 174)))
        .corner_radius(4)
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_max_width(260.0);
            ui.label(
                RichText::new(title)
                    .size(14.0)
                    .strong()
                    .color(Color32::from_rgb(255, 214, 105)),
            );
            ui.add_space(4.0);
            ui.label(RichText::new(body).size(13.0).color(text()));
        });
}
