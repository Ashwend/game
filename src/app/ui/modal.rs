use bevy_egui::egui::{self, Align2, Color32, Frame, Id, Margin, Order, RichText, Sense, Stroke};

use super::theme::{self, ButtonKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfirmationChoice {
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ConfirmationModalOutput {
    pub(super) choice: Option<ConfirmationChoice>,
    pub(super) finished_closing: bool,
}

pub(super) fn confirmation_modal(
    ctx: &egui::Context,
    id: &'static str,
    title: &str,
    body: &str,
    confirm_label: &str,
    cancel_label: &str,
    open: bool,
) -> ConfirmationModalOutput {
    let id = Id::new(id);
    let animation = ctx.animate_bool_with_time(id.with("animation"), open, 0.16);
    if animation > 0.0 && animation < 1.0 {
        ctx.request_repaint();
    }

    if !open && animation <= 0.01 {
        return ConfirmationModalOutput {
            choice: None,
            finished_closing: true,
        };
    }

    let screen_rect = ctx.content_rect();
    let backdrop_response = egui::Area::new(id.with("backdrop"))
        .order(Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let local_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, screen_rect.size());
            let response = ui.allocate_rect(local_rect, Sense::click());
            ui.painter().rect_filled(
                local_rect,
                0,
                Color32::from_rgba_unmultiplied(1, 3, 8, (190.0 * animation) as u8),
            );
            response
        })
        .inner;

    let panel_width = screen_rect.width().clamp(320.0, 410.0);
    let mut choice = None;
    let panel_response = egui::Area::new(id.with("panel"))
        .order(Order::Tooltip)
        .anchor(
            Align2::CENTER_CENTER,
            [0.0, 18.0 * (1.0 - animation.clamp(0.0, 1.0))],
        )
        .show(ctx, |ui| {
            ui.set_width(panel_width);
            ui.multiply_opacity(animation);
            Frame::NONE
                .fill(Color32::from_rgba_unmultiplied(12, 17, 23, 246))
                .stroke(Stroke::new(1.0, theme::panel_stroke()))
                .corner_radius(7)
                .inner_margin(Margin::symmetric(24, 22))
                .show(ui, |ui| {
                    ui.set_width(panel_width - 48.0);
                    ui.label(theme::section(title));
                    ui.add_space(8.0);
                    ui.label(RichText::new(body).size(14.0).color(theme::text()));
                    ui.add_space(18.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::compact_button(ui, confirm_label, ButtonKind::Danger, 92.0)
                            .clicked()
                        {
                            choice = Some(ConfirmationChoice::Confirm);
                        }
                        if theme::compact_button(ui, cancel_label, ButtonKind::Secondary, 92.0)
                            .clicked()
                        {
                            choice = Some(ConfirmationChoice::Cancel);
                        }
                    });
                });
        })
        .response;

    if open && choice.is_none() && backdrop_response.clicked() {
        let clicked_outside_panel = ctx.input(|input| {
            input
                .pointer
                .interact_pos()
                .is_some_and(|position| !panel_response.rect.contains(position))
        });
        if clicked_outside_panel {
            choice = Some(ConfirmationChoice::Cancel);
        }
    }

    ConfirmationModalOutput {
        choice,
        finished_closing: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            ..Default::default()
        }
    }

    #[test]
    fn closed_modal_finishes_without_rendering_panel() {
        let ctx = egui::Context::default();
        let output =
            confirmation_modal(&ctx, "closed", "Title", "Body", "Confirm", "Cancel", false);

        assert!(output.finished_closing);
        assert!(output.choice.is_none());
    }

    #[test]
    fn open_modal_renders_and_keeps_waiting_for_choice() {
        let ctx = egui::Context::default();
        let mut output = None;

        let _ = ctx.run(raw_input(), |ctx| {
            output = Some(confirmation_modal(
                ctx, "open", "Title", "Body", "Confirm", "Cancel", true,
            ));
        });

        let output = output.expect("modal output should be set");
        assert!(!output.finished_closing);
        assert!(output.choice.is_none());
    }
}
