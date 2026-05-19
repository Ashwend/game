use bevy_egui::egui::{self, Color32, Frame, Label, Margin, RichText, Stroke, vec2};

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

pub(in crate::app::ui) fn anchored_wow_tooltip(
    ctx: &egui::Context,
    id: impl std::hash::Hash,
    anchor: egui::Pos2,
    title: &str,
    body: &str,
) {
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(anchor + vec2(16.0, 18.0))
        .show(ctx, |ui| {
            draw_wow_tooltip(ui, title, body);
        });
}

fn draw_wow_tooltip(ui: &mut egui::Ui, title: &str, body: &str) {
    Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(4, 6, 12, 244))
        .stroke(Stroke::new(1.0, Color32::from_rgb(78, 112, 174)))
        .corner_radius(4)
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_max_width(260.0);
            // Tooltips for world objects must not behave like selectable text;
            // dragging across one shouldn't start a text selection.
            ui.add(
                Label::new(
                    RichText::new(title)
                        .size(14.0)
                        .strong()
                        .color(Color32::from_rgb(255, 214, 105)),
                )
                .selectable(false),
            );
            ui.add_space(4.0);
            ui.add(Label::new(RichText::new(body).size(13.0).color(text())).selectable(false));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_body_renders_in_headless_context() {
        let ctx = egui::Context::default();

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_wow_tooltip(ui, "Title", "Body");
                    anchored_wow_tooltip(
                        ctx,
                        "anchored_test_tooltip",
                        egui::pos2(12.0, 18.0),
                        "Title",
                        "Body",
                    );
                    let response =
                        ui.allocate_response(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    let returned = wow_tooltip(response, "Title", "Body");
                    assert!(!returned.clicked());
                });
            },
        );
    }
}
