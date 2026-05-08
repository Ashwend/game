use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Id, Margin, Order, Stroke,
    TextStyle, vec2,
};

use super::{
    accent, accent_dark, button_fill, button_hover_fill, button_stroke, input_fill, panel_fill,
    panel_stroke, text,
};

pub(in crate::app::ui) fn apply_game_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(28.0, FontFamily::Proportional),
        );
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        );

        style.spacing.item_spacing = vec2(10.0, 8.0);
        style.spacing.button_padding = vec2(16.0, 9.0);
        style.spacing.window_margin = Margin::same(18);
        style.visuals.override_text_color = Some(text());
        style.visuals.window_fill = panel_fill();
        style.visuals.panel_fill = Color32::TRANSPARENT;
        style.visuals.extreme_bg_color = input_fill();
        style.visuals.text_edit_bg_color = Some(input_fill());
        style.visuals.window_corner_radius = CornerRadius::same(7);
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text());
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, panel_stroke());
        style.visuals.widgets.inactive.bg_fill = button_fill();
        style.visuals.widgets.inactive.weak_bg_fill = button_fill();
        style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, button_stroke());
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text());
        style.visuals.widgets.hovered.bg_fill = button_hover_fill();
        style.visuals.widgets.hovered.weak_bg_fill = button_hover_fill();
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, accent());
        style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        style.visuals.widgets.active.bg_fill = accent_dark();
        style.visuals.widgets.active.weak_bg_fill = accent_dark();
        style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, accent());
        style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    });
}

pub(in crate::app::ui) fn screen_scrim(ctx: &egui::Context, id: &'static str, alpha: u8) {
    screen_fill(ctx, id, Color32::from_rgba_unmultiplied(2, 4, 7, alpha));
}

pub(in crate::app::ui) fn backdrop_cover(ctx: &egui::Context, alpha: u8) {
    if alpha == 0 {
        return;
    }

    screen_fill(
        ctx,
        "menu_backdrop_blur_cover",
        Color32::from_rgba_unmultiplied(0, 0, 0, alpha),
    );
}

fn screen_fill(ctx: &egui::Context, id: &'static str, fill: Color32) {
    let rect = ctx.content_rect();
    egui::Area::new(Id::new(id))
        .order(Order::Background)
        .interactable(false)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            let local_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, rect.size());
            ui.allocate_rect(local_rect, egui::Sense::hover());
            ui.painter().rect_filled(local_rect, 0, fill);
        });
}

pub(in crate::app::ui) fn panel_frame() -> Frame {
    Frame::NONE
        .fill(panel_fill())
        .stroke(Stroke::new(1.0, panel_stroke()))
        .corner_radius(7)
        .inner_margin(Margin::symmetric(24, 22))
}

pub(in crate::app::ui) fn inset_frame() -> Frame {
    Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(7, 10, 14, 206))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(90, 108, 128, 92),
        ))
        .corner_radius(5)
        .inner_margin(Margin::symmetric(14, 12))
}

pub(in crate::app::ui) fn anchored_panel(
    ctx: &egui::Context,
    id: &'static str,
    desired_width: f32,
    anchor: Align2,
    offset: [f32; 2],
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let screen_width = ctx.content_rect().width();
    let width = desired_width.min((screen_width - 56.0).max(300.0));
    egui::Area::new(Id::new(id))
        .order(Order::Foreground)
        .anchor(anchor, offset)
        .show(ctx, |ui| {
            ui.set_width(width);
            panel_frame().show(ui, |ui| {
                ui.set_width(width - 48.0);
                add_contents(ui);
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        }
    }

    #[test]
    fn style_and_frames_apply_game_palette() {
        let ctx = egui::Context::default();
        apply_game_style(&ctx);

        let style = ctx.style();
        assert_eq!(style.visuals.override_text_color, Some(text()));
        assert_eq!(style.visuals.window_fill, panel_fill());
        assert_eq!(style.visuals.widgets.active.bg_fill, accent_dark());

        let panel = panel_frame();
        assert_eq!(panel.fill, panel_fill());
        let inset = inset_frame();
        assert_ne!(inset.fill, Color32::TRANSPARENT);
    }

    #[test]
    fn scrim_and_panel_render_in_headless_context() {
        let ctx = egui::Context::default();

        let output = ctx.run(input(), |ctx| {
            screen_scrim(ctx, "test_scrim", 120);
            backdrop_cover(ctx, 180);
            anchored_panel(
                ctx,
                "test_panel",
                500.0,
                Align2::CENTER_CENTER,
                [0.0, 0.0],
                |ui| {
                    ui.label("content");
                },
            );
        });

        assert!(!output.shapes.is_empty());
    }
}
