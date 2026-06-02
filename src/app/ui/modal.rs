use bevy_egui::egui::{
    self, Align2, Color32, Frame, Id, LayerId, Margin, Order, RichText, Sense, Stroke,
    emath::TSTransform,
};

use super::theme::{self, ButtonKind};

/// Where the panel sits vertically as it animates in. The scrim has already
/// faded, the panel rises this many pixels into place.
const MODAL_SLIDE_OFFSET_PX: f32 = 18.0;
/// Minimum scale at the start of the open animation. 0.94 is just enough to
/// be felt as a "rising into place" gesture without making the modal feel
/// like it's flying in from the back.
const MODAL_MIN_SCALE: f32 = 0.94;
/// How long the open/close transition lasts. The full slide, fade, and
/// scale share the same window so the three motions feel coordinated.
const MODAL_ANIMATION_SECS: f32 = 0.16;

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

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

pub(super) struct ModalShellOutput<T> {
    pub(super) choice: Option<T>,
    pub(super) finished_closing: bool,
    pub(super) confirm_shortcut_pressed: bool,
    pub(super) clicked_outside: bool,
}

pub(in crate::app::ui) fn confirm_shortcut_pressed(ctx: &egui::Context) -> bool {
    ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
}

/// Full-screen scrim used as the dim layer underneath pause / inventory /
/// modal overlays. Returns the response so callers can react to a click on
/// empty space (e.g. close-on-outside). The scrim itself is opaque to clicks
///, that's the whole point of having one, so layering multiple of these
/// for different overlays is fine as long as each gets a unique `id`.
pub(in crate::app::ui) fn backdrop_layer(
    ctx: &egui::Context,
    id: &'static str,
    order: Order,
    color: Color32,
) -> egui::Response {
    let screen_rect = ctx.content_rect();
    egui::Area::new(Id::new(id))
        .order(order)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let local_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, screen_rect.size());
            let response = ui.allocate_rect(local_rect, Sense::click());
            ui.painter().rect_filled(local_rect, 0.0, color);
            response
        })
        .inner
}

pub(super) fn modal_shell<T>(
    ctx: &egui::Context,
    id: &'static str,
    open: bool,
    min_width: f32,
    max_width: f32,
    add_contents: impl FnOnce(&mut egui::Ui, &mut Option<T>),
) -> ModalShellOutput<T> {
    let id = Id::new(id);
    let raw_animation =
        ctx.animate_bool_with_time(id.with("animation"), open, MODAL_ANIMATION_SECS);
    if raw_animation > 0.0 && raw_animation < 1.0 {
        ctx.request_repaint();
    }

    if !open && raw_animation <= 0.01 {
        return ModalShellOutput {
            choice: None,
            finished_closing: true,
            confirm_shortcut_pressed: false,
            clicked_outside: false,
        };
    }

    // Ease the linear `animate_bool` curve so the slide-in/scale-up tapers
    // gently into rest rather than coming to an abrupt stop. The backdrop
    // alpha still rides the raw curve so the scrim never lingers behind a
    // panel that is already invisible during the closing tail.
    let eased = ease_out_cubic(raw_animation);

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
                Color32::from_rgba_unmultiplied(1, 3, 8, (190.0 * raw_animation) as u8),
            );
            response
        })
        .inner;

    let panel_width = screen_rect.width().clamp(min_width, max_width);
    let mut choice = None;
    let panel_layer = LayerId::new(Order::Tooltip, id.with("panel"));
    let panel_response = egui::Area::new(id.with("panel"))
        .order(Order::Tooltip)
        .anchor(
            Align2::CENTER_CENTER,
            [0.0, MODAL_SLIDE_OFFSET_PX * (1.0 - eased)],
        )
        .show(ctx, |ui| {
            ui.set_width(panel_width);
            ui.multiply_opacity(eased);
            Frame::NONE
                .fill(Color32::from_rgba_unmultiplied(12, 17, 23, 246))
                .stroke(Stroke::new(1.0, theme::panel_stroke()))
                .corner_radius(7)
                .inner_margin(Margin::symmetric(24, 22))
                .show(ui, |ui| {
                    ui.set_width(panel_width - 48.0);
                    add_contents(ui, &mut choice);
                });
        })
        .response;

    // Apply a one-frame scale around the panel center on top of the slide
    // + fade. `transform_layer_shapes` retroactively scales every shape
    // that landed in the panel layer this frame. The interactive rect
    // stays in original coordinates, which matters for very early clicks
    //, but the 0.94 → 1.0 range is small enough that the visual and
    // interactive rectangles align within a few pixels at the worst point.
    let scale = MODAL_MIN_SCALE + (1.0 - MODAL_MIN_SCALE) * eased;
    if (scale - 1.0).abs() > f32::EPSILON {
        let center = panel_response.rect.center().to_vec2();
        let transform = TSTransform::new(center * (1.0 - scale), scale);
        ctx.transform_layer_shapes(panel_layer, transform);
    }

    let confirm_shortcut_pressed = open && choice.is_none() && confirm_shortcut_pressed(ctx);
    let clicked_outside = open
        && choice.is_none()
        && !confirm_shortcut_pressed
        && backdrop_response.clicked()
        && ctx.input(|input| {
            input
                .pointer
                .interact_pos()
                .is_some_and(|position| !panel_response.rect.contains(position))
        });

    ModalShellOutput {
        choice,
        finished_closing: false,
        confirm_shortcut_pressed,
        clicked_outside,
    }
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
    let output = modal_shell(ctx, id, open, 320.0, 410.0, |ui, choice| {
        ui.label(theme::section(title));
        ui.add_space(8.0);
        ui.label(RichText::new(body).size(14.0).color(theme::text()));
        ui.add_space(18.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::compact_button(ui, confirm_label, ButtonKind::Danger, 92.0).clicked() {
                *choice = Some(ConfirmationChoice::Confirm);
            }
            if theme::compact_button(ui, cancel_label, ButtonKind::Secondary, 92.0).clicked() {
                *choice = Some(ConfirmationChoice::Cancel);
            }
        });
    });

    let mut choice = output.choice;
    if choice.is_none() && output.confirm_shortcut_pressed {
        choice = Some(ConfirmationChoice::Confirm);
    }
    if choice.is_none() && output.clicked_outside {
        choice = Some(ConfirmationChoice::Cancel);
    }

    ConfirmationModalOutput {
        choice,
        finished_closing: output.finished_closing,
    }
}

pub(super) fn notice_modal(
    ctx: &egui::Context,
    id: &'static str,
    title: &str,
    body: &str,
    confirm_label: &str,
    open: bool,
) -> ConfirmationModalOutput {
    let output = modal_shell(ctx, id, open, 320.0, 410.0, |ui, choice| {
        ui.label(theme::section(title));
        ui.add_space(8.0);
        ui.label(RichText::new(body).size(14.0).color(theme::text()));
        ui.add_space(18.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::compact_button(ui, confirm_label, ButtonKind::Primary, 92.0).clicked() {
                *choice = Some(ConfirmationChoice::Confirm);
            }
        });
    });

    let mut choice = output.choice;
    if choice.is_none() && (output.confirm_shortcut_pressed || output.clicked_outside) {
        choice = Some(ConfirmationChoice::Confirm);
    }

    ConfirmationModalOutput {
        choice,
        finished_closing: output.finished_closing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_input() -> egui::RawInput {
        raw_input_with_events(Vec::new())
    }

    fn raw_input_with_events(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn key_press(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    #[test]
    fn ease_out_cubic_starts_fast_and_settles_at_one() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        // Past halfway the curve should still be moving but well above the
        // linear midpoint, that's the whole point of "ease-out".
        let half = ease_out_cubic(0.5);
        assert!(half > 0.5);
        assert!(half < 1.0);
        // Out-of-range inputs are clamped rather than overshooting.
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
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

    #[test]
    fn enter_confirms_open_modal() {
        let ctx = egui::Context::default();
        let mut output = None;

        let _ = ctx.run(
            raw_input_with_events(vec![key_press(egui::Key::Enter)]),
            |ctx| {
                output = Some(confirmation_modal(
                    ctx, "enter", "Title", "Body", "Confirm", "Cancel", true,
                ));
            },
        );

        let output = output.expect("modal output should be set");
        assert_eq!(output.choice, Some(ConfirmationChoice::Confirm));
        assert!(!output.finished_closing);
    }
}
