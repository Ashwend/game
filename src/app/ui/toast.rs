use bevy_egui::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Id, LayerId, Order, Pos2, Rect, Stroke,
    StrokeKind,
    text::{LayoutJob, TextFormat, TextWrapping},
};

use crate::{
    app::state::{TOAST_FADE_SECONDS, TOAST_VISIBLE_SECONDS, Toast, ToastState},
    protocol::ToastKind,
};

use super::theme::{self, COMPACT_ROW_HEIGHT};

const TOAST_MAX_WIDTH: f32 = 280.0;
/// Floor for the toast width on cramped screens. Anything narrower would
/// only fit a couple of glyphs before the ellipsis kicks in, so we stop
/// shrinking here and let the bar overlap rather than render an unreadable
/// stub.
const TOAST_MIN_WIDTH: f32 = 140.0;
const TOAST_HEIGHT: f32 = COMPACT_ROW_HEIGHT;
const TOAST_GAP: f32 = 6.0;
const RIGHT_MARGIN: f32 = 18.0;
const BOTTOM_MARGIN: f32 = 64.0;
const SLIDE_DISTANCE: f32 = 38.0;
const TEXT_LEFT_PADDING: f32 = 14.0;
const TEXT_RIGHT_PADDING: f32 = 10.0;
/// Minimum horizontal gap between the toast stack's left edge and the
/// actionbar's outer-right edge.
const TOAST_ACTIONBAR_GAP: f32 = 16.0;
const TOAST_FONT_SIZE: f32 = 13.5;
const CORNER_RADIUS: u8 = 5;

pub(super) fn toast_ui(ctx: &egui::Context, toasts: &ToastState, actionbar_rect: Option<Rect>) {
    if toasts.is_empty() {
        return;
    }

    let screen_rect = ctx.content_rect();
    let right_edge = screen_rect.right() - RIGHT_MARGIN;
    let bottom_edge = screen_rect.bottom() - BOTTOM_MARGIN;
    let toast_width = effective_toast_width(right_edge, actionbar_rect);
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("toast_stack")));

    let mut cumulative = 0.0_f32;
    let mut needs_repaint = false;

    // Visible is ordered oldest → newest; iterate reversed so the newest toast
    // sits at the bottom of the stack (closest to the screen edge) and older
    // ones rise above it.
    for toast in toasts.visible().collect::<Vec<_>>().into_iter().rev() {
        let lifecycle = toast_lifecycle(toast.age);

        let y_bottom = bottom_edge - cumulative;
        let y_top = y_bottom - TOAST_HEIGHT;
        let x_right = right_edge + lifecycle.slide_x;
        let x_left = x_right - toast_width;
        let rect = Rect::from_min_max(Pos2::new(x_left, y_top), Pos2::new(x_right, y_bottom));

        paint_toast(ctx, &painter, toast, rect, lifecycle.alpha);

        // The slot the toast occupies in the layout shrinks during exit so
        // older toasts above it animate downward. The toast's painted rect
        // is fixed at `TOAST_HEIGHT`, so the visual itself doesn't deform,
        // it just slides off to the right while its row collapses underneath.
        cumulative += (TOAST_HEIGHT + TOAST_GAP) * lifecycle.height_factor;

        if lifecycle.animating {
            needs_repaint = true;
        }
    }

    if needs_repaint {
        ctx.request_repaint();
    }
}

/// Shrink the toast width so its left edge stays at least
/// `TOAST_ACTIONBAR_GAP` to the right of the actionbar's outer-right edge.
/// Falls back to the full width when the actionbar hasn't been laid out yet.
fn effective_toast_width(right_edge: f32, actionbar_rect: Option<Rect>) -> f32 {
    let Some(rect) = actionbar_rect else {
        return TOAST_MAX_WIDTH;
    };
    let usable = right_edge - rect.right() - TOAST_ACTIONBAR_GAP;
    usable.clamp(TOAST_MIN_WIDTH, TOAST_MAX_WIDTH)
}

#[derive(Debug, Clone, Copy)]
struct Lifecycle {
    /// 0.0 → fully hidden, 1.0 → fully visible.
    alpha: f32,
    /// 0.0 → no slot taken (collapsed), 1.0 → full slot. Drives stacking only;
    /// the toast's painted size stays constant.
    height_factor: f32,
    /// Horizontal offset in pixels. Negative = nudged left of the resting
    /// position (enter phase), positive = past the right anchor (exit phase).
    slide_x: f32,
    animating: bool,
}

fn toast_lifecycle(age: f32) -> Lifecycle {
    let raw_appear = (age / TOAST_FADE_SECONDS).clamp(0.0, 1.0);
    let raw_exit = if age <= TOAST_VISIBLE_SECONDS {
        0.0
    } else {
        ((age - TOAST_VISIBLE_SECONDS) / TOAST_FADE_SECONDS).clamp(0.0, 1.0)
    };

    let appear = ease_out_cubic(raw_appear);
    let exit_slide = ease_in_cubic(raw_exit);
    let exit_collapse = ease_in_out_cubic(raw_exit);

    let alpha = appear * (1.0 - exit_slide);
    let height_factor = appear * (1.0 - exit_collapse);
    let slide_x = (1.0 - appear) * (-SLIDE_DISTANCE) + exit_slide * SLIDE_DISTANCE;
    let animating = (raw_appear > 0.0 && raw_appear < 1.0) || (raw_exit > 0.0 && raw_exit < 1.0);

    Lifecycle {
        alpha,
        height_factor,
        slide_x,
        animating,
    }
}

fn paint_toast(
    ctx: &egui::Context,
    painter: &egui::Painter,
    toast: &Toast,
    rect: Rect,
    alpha: f32,
) {
    if alpha <= 0.001 {
        return;
    }

    let corner = CornerRadius::same(CORNER_RADIUS);
    let panel_color = with_alpha(theme::panel_fill(), alpha);
    let stroke_color = with_alpha(theme::panel_stroke(), alpha);
    let text_color = with_alpha(text_color_for_kind(toast.kind), alpha);

    painter.rect_filled(rect, corner, panel_color);
    painter.rect_stroke(
        rect,
        corner,
        Stroke::new(1.0, stroke_color),
        StrokeKind::Inside,
    );

    let inner_highlight_alpha = (10.0 * alpha) as u8;
    if inner_highlight_alpha > 0 {
        painter.rect_stroke(
            rect.shrink(0.5),
            corner,
            Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, inner_highlight_alpha),
            ),
            StrokeKind::Inside,
        );
    }

    let text_pos = Pos2::new(rect.left() + TEXT_LEFT_PADDING, rect.center().y);
    let text_max_width = (rect.width() - TEXT_LEFT_PADDING - TEXT_RIGHT_PADDING).max(0.0);
    let galley = layout_single_line(ctx, &toast.text, text_color, text_max_width);
    let galley_pos = Pos2::new(text_pos.x, text_pos.y - galley.size().y * 0.5);
    painter.galley(galley_pos, galley, text_color);
}

/// Lay the toast text out as a single line, truncating with an ellipsis when
/// it doesn't fit in the available width. The single-line + break-anywhere
/// combo is what egui's docs recommend for one-row elision.
fn layout_single_line(
    ctx: &egui::Context,
    text: &str,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: FontId::new(TOAST_FONT_SIZE, FontFamily::Proportional),
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ctx.fonts_mut(|fonts| fonts.layout_job(job))
}

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    let [r, g, b, a] = color.to_array();
    Color32::from_rgba_unmultiplied(r, g, b, (a as f32 * alpha.clamp(0.0, 1.0)) as u8)
}

/// Per-kind text color. Tuned to read cleanly against the dark panel fill
/// without being saturated enough to feel garish at a glance. The kind is
/// communicated by the message hue alone, no border, dot, or badge.
fn text_color_for_kind(kind: ToastKind) -> Color32 {
    match kind {
        ToastKind::Info => Color32::from_rgb(206, 220, 234),
        ToastKind::Success => Color32::from_rgb(168, 215, 168),
        ToastKind::Warning => Color32::from_rgb(228, 196, 134),
        ToastKind::Error => Color32::from_rgb(228, 154, 154),
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

fn ease_in_cubic(t: f32) -> f32 {
    t * t * t
}

fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let f = 2.0 * t - 2.0;
        1.0 + f * f * f * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_starts_invisible_collapsed_and_offset_left() {
        let l = toast_lifecycle(0.0);
        assert!(l.alpha <= 0.001);
        assert!(l.height_factor <= 0.001);
        assert!(l.slide_x < -SLIDE_DISTANCE * 0.9);
    }

    #[test]
    fn lifecycle_settles_at_full_visibility_after_fade_in() {
        let l = toast_lifecycle(TOAST_FADE_SECONDS);
        assert!(l.alpha >= 0.999);
        assert!(l.height_factor >= 0.999);
        assert!(l.slide_x.abs() <= 0.001);
        assert!(!l.animating);
    }

    #[test]
    fn lifecycle_stays_steady_during_visible_window() {
        let mid = TOAST_VISIBLE_SECONDS * 0.5;
        let l = toast_lifecycle(mid);
        assert!(l.alpha >= 0.999);
        assert!(l.height_factor >= 0.999);
        assert!(l.slide_x.abs() <= 0.001);
    }

    #[test]
    fn lifecycle_slides_right_and_collapses_during_exit() {
        let exiting = TOAST_VISIBLE_SECONDS + TOAST_FADE_SECONDS * 0.5;
        let l = toast_lifecycle(exiting);
        assert!(l.alpha > 0.0 && l.alpha < 1.0);
        assert!(l.height_factor > 0.0 && l.height_factor < 1.0);
        assert!(l.slide_x > 0.0);
    }

    #[test]
    fn lifecycle_ends_hidden_and_collapsed_with_rightward_offset() {
        let end = TOAST_VISIBLE_SECONDS + TOAST_FADE_SECONDS;
        let l = toast_lifecycle(end);
        assert!(l.alpha <= 0.001);
        assert!(l.height_factor <= 0.001);
        assert!(l.slide_x >= SLIDE_DISTANCE * 0.99);
    }

    #[test]
    fn kind_text_colors_are_distinct() {
        let info = text_color_for_kind(ToastKind::Info);
        let success = text_color_for_kind(ToastKind::Success);
        let warning = text_color_for_kind(ToastKind::Warning);
        let error = text_color_for_kind(ToastKind::Error);
        assert_ne!(info, success);
        assert_ne!(success, warning);
        assert_ne!(warning, error);
    }

    #[test]
    fn effective_toast_width_uses_max_when_actionbar_unknown() {
        let right_edge = 1280.0;
        assert_eq!(effective_toast_width(right_edge, None), TOAST_MAX_WIDTH);
    }

    #[test]
    fn effective_toast_width_shrinks_to_keep_actionbar_gap() {
        let right_edge = 1280.0;
        let crowded = Rect::from_min_max(Pos2::new(380.0, 600.0), Pos2::new(1040.0, 700.0));
        let width = effective_toast_width(right_edge, Some(crowded));
        assert!(width < TOAST_MAX_WIDTH);
        // The toast left edge must sit `TOAST_ACTIONBAR_GAP` past the bar.
        let toast_left = right_edge - width;
        assert!((toast_left - crowded.right() - TOAST_ACTIONBAR_GAP).abs() <= 0.001);
    }

    #[test]
    fn effective_toast_width_floors_at_minimum() {
        let right_edge = 800.0;
        let cramped = Rect::from_min_max(Pos2::new(120.0, 500.0), Pos2::new(760.0, 600.0));
        assert_eq!(
            effective_toast_width(right_edge, Some(cramped)),
            TOAST_MIN_WIDTH
        );
    }

    #[test]
    fn long_toast_text_is_truncated_with_ellipsis_in_narrow_width() {
        let ctx = egui::Context::default();
        let color = Color32::WHITE;
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let galley = layout_single_line(
            &ctx,
            "this is a very long toast message that should not fit in a small width",
            color,
            80.0,
        );
        assert!(galley.elided, "narrow toast should mark galley as elided");
        assert!(galley.rect.width() <= 80.0 + 0.5);
    }

    #[test]
    fn short_toast_text_is_not_truncated_with_room_to_spare() {
        let ctx = egui::Context::default();
        let color = Color32::WHITE;
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let galley = layout_single_line(&ctx, "ok", color, 200.0);
        assert!(!galley.elided);
    }
}
