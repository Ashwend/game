//! Always-on top-right stack of crafting progress cards.
//!
//! Split from the crafting browser ([`super::crafting`]) because the HUD
//! survives closing the modal, keeping the recipe-browser layout
//! separate from the persistent queue overlay makes both easier to
//! reason about.

use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Id, Order, Pos2, Rect, Sense, Stroke,
    StrokeKind, Vec2,
};

use crate::{
    app::{
        state::{
            ClientRuntime, CraftingHudState, ErrorToastSink, LocalPlayerState, ProgressBaseline,
        },
        systems::send_crafting_command,
    },
    items::{ItemTint, item_definition},
    protocol::{CraftingCommand, CraftingJob, SERVER_TICK_RATE_HZ},
};

use super::theme;

const QUEUE_CARD_WIDTH: f32 = 280.0;
const QUEUE_CARD_HEIGHT: f32 = 56.0;
const QUEUE_CARD_GAP: f32 = 8.0;
const QUEUE_TOP_MARGIN: f32 = 24.0;
const QUEUE_RIGHT_MARGIN: f32 = 24.0;
const QUEUE_CANCEL_BUTTON_SIZE: f32 = 22.0;
/// Number of queue cards rendered before the HUD switches to a compact
/// "+N more" overflow bar. Beyond this we don't paint per-job cards at
/// all, the always-open recipe browser shows the full queue if the
/// player wants the rest.
const QUEUE_VISIBLE_CARDS: usize = 3;
/// Height of the compact "+N more" overflow indicator drawn below the
/// last visible queue card. Shorter than a full card on purpose so it
/// reads as a secondary "there's more" hint, not another job entry.
const QUEUE_OVERFLOW_BAR_HEIGHT: f32 = 22.0;

pub(super) fn crafting_queue_hud(
    ctx: &egui::Context,
    runtime: &mut ClientRuntime,
    local_player: &LocalPlayerState,
    hud_state: &mut CraftingHudState,
    error_toasts: &mut dyn ErrorToastSink,
) {
    let Some(jobs) = local_player
        .private
        .as_ref()
        .map(|p| p.crafting.jobs.clone())
    else {
        // Clear any stale baselines so a future job_id collision (the
        // server's id allocator wraps eventually) can't inherit a wrong
        // observation timestamp.
        hud_state.progress.clear();
        return;
    };
    if jobs.is_empty() {
        hud_state.progress.clear();
        return;
    }

    let now_secs = ctx.input(|input| input.time);
    // Forget baselines for jobs that left the queue (completed or
    // cancelled).
    {
        let live: std::collections::HashSet<_> = jobs.iter().map(|job| job.job_id).collect();
        hud_state.progress.retain(|job_id, _| live.contains(job_id));
    }

    let screen_rect = ctx.content_rect();
    let card_x_right = screen_rect.right() - QUEUE_RIGHT_MARGIN;
    let card_x_left = card_x_right - QUEUE_CARD_WIDTH;

    let mut cancel_target: Option<crate::protocol::CraftingJobId> = None;
    // Egui repaints on input and animation. The bar interpolation is
    // continuous, so we need to ask for the next frame regardless of
    // whether anything else moved.
    ctx.request_repaint();

    let visible_count = jobs.len().min(QUEUE_VISIBLE_CARDS);
    let hidden_count = jobs.len().saturating_sub(QUEUE_VISIBLE_CARDS);
    for (index, job) in jobs.iter().enumerate() {
        let alpha = queue_card_alpha(index);
        if alpha <= 0.0 {
            continue;
        }
        let is_head = index == 0;
        let fraction = smoothed_fraction(hud_state, job, now_secs, is_head);
        let y_top = screen_rect.top()
            + QUEUE_TOP_MARGIN
            + index as f32 * (QUEUE_CARD_HEIGHT + QUEUE_CARD_GAP);
        let rect = Rect::from_min_size(
            Pos2::new(card_x_left, y_top),
            Vec2::new(QUEUE_CARD_WIDTH, QUEUE_CARD_HEIGHT),
        );
        let area_response = egui::Area::new(Id::new(("crafting_queue_card", job.job_id)))
            .order(Order::Foreground)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                ui.multiply_opacity(alpha);
                draw_queue_card(ui, rect, job, is_head, fraction)
            });
        if area_response.inner.cancel_clicked {
            cancel_target = Some(job.job_id);
        }
    }

    if hidden_count > 0 && visible_count > 0 {
        let last_visible_bottom = screen_rect.top()
            + QUEUE_TOP_MARGIN
            + (visible_count - 1) as f32 * (QUEUE_CARD_HEIGHT + QUEUE_CARD_GAP)
            + QUEUE_CARD_HEIGHT;
        let bar_rect = Rect::from_min_size(
            Pos2::new(card_x_left, last_visible_bottom + QUEUE_CARD_GAP * 0.5),
            Vec2::new(QUEUE_CARD_WIDTH, QUEUE_OVERFLOW_BAR_HEIGHT),
        );
        egui::Area::new(Id::new("crafting_queue_overflow"))
            .order(Order::Foreground)
            .fixed_pos(bar_rect.min)
            .show(ctx, |ui| {
                draw_queue_overflow(ui, bar_rect, hidden_count);
            });
    }

    if let Some(job_id) = cancel_target {
        // Mark before sending so the job's disappearance from the next
        // snapshot reads as a cancel, not a completion chime.
        hud_state.note_cancel_requested(job_id);
        send_crafting_command(runtime, error_toasts, CraftingCommand::Cancel { job_id });
    }
}

/// Compute the progress fraction the card should render this frame.
///
/// For the head job: anchor a baseline the first time we see a given
/// `progress_ticks` value, then advance the fraction off the wall clock
/// at `SERVER_TICK_RATE_HZ` until the next snapshot rebases it. The
/// final clamp at 1.0 keeps a stale or slow-arriving "completed"
/// snapshot from painting past the bar's right edge.
///
/// Queued (non-head) jobs always render at 0, the server doesn't
/// advance them, so neither should we.
fn smoothed_fraction(
    hud_state: &mut CraftingHudState,
    job: &CraftingJob,
    now_secs: f64,
    is_head: bool,
) -> f32 {
    if !is_head {
        hud_state
            .progress
            .insert(job.job_id, baseline_from(job, now_secs));
        return 0.0;
    }

    let entry = hud_state.progress.entry(job.job_id);
    let baseline = match entry {
        std::collections::hash_map::Entry::Occupied(mut slot) => {
            let current = slot.get_mut();
            if current.observed_ticks != job.progress_ticks
                || current.total_ticks != job.total_ticks
            {
                *current = baseline_from(job, now_secs);
            }
            *current
        }
        std::collections::hash_map::Entry::Vacant(slot) => {
            *slot.insert(baseline_from(job, now_secs))
        }
    };

    if baseline.total_ticks == 0 {
        return 1.0;
    }
    let elapsed_ticks =
        (now_secs - baseline.observed_at_secs).max(0.0) as f32 * SERVER_TICK_RATE_HZ;
    let projected = baseline.observed_ticks as f32 + elapsed_ticks;
    (projected / baseline.total_ticks as f32).clamp(0.0, 1.0)
}

/// Whether a queue card at `index` is rendered. Cards within the
/// visible window paint at full opacity; anything past the window is
/// represented by the compact "+N more" overflow bar instead.
fn queue_card_alpha(index: usize) -> f32 {
    if index < QUEUE_VISIBLE_CARDS {
        1.0
    } else {
        0.0
    }
}

fn baseline_from(job: &CraftingJob, now_secs: f64) -> ProgressBaseline {
    ProgressBaseline {
        observed_ticks: job.progress_ticks,
        total_ticks: job.total_ticks,
        observed_at_secs: now_secs,
    }
}

struct QueueCardResponse {
    cancel_clicked: bool,
}

/// Slim "+N more" pill drawn under the last visible queue card when the
/// queue runs deeper than [`QUEUE_VISIBLE_CARDS`]. Non-interactive on
/// purpose, clicking it doesn't open or expand anything, since the
/// crafting modal already exposes the full queue count. The goal is a
/// silent visual hint, not another button.
fn draw_queue_overflow(ui: &mut egui::Ui, rect: Rect, hidden_count: usize) {
    let _ = ui.allocate_rect(rect, Sense::hover());
    let painter = ui.painter().clone();

    let corner = CornerRadius::same(4);
    let fill = theme::panel_fill().gamma_multiply(0.65);
    painter.rect_filled(rect, corner, fill);
    painter.rect_stroke(
        rect,
        corner,
        Stroke::new(1.0, theme::panel_stroke()),
        StrokeKind::Inside,
    );

    let label = if hidden_count == 1 {
        "+1 more queued".to_owned()
    } else {
        format!("+{hidden_count} more queued")
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::new(11.5, FontFamily::Proportional),
        theme::muted_text(),
    );
}

fn draw_queue_card(
    ui: &mut egui::Ui,
    rect: Rect,
    job: &CraftingJob,
    is_head: bool,
    fraction: f32,
) -> QueueCardResponse {
    let _ = ui.allocate_rect(rect, Sense::hover());
    let painter = ui.painter().clone();

    let corner = CornerRadius::same(5);
    painter.rect_filled(rect, corner, theme::panel_fill());
    painter.rect_stroke(
        rect,
        corner,
        Stroke::new(1.0, theme::panel_stroke()),
        StrokeKind::Inside,
    );

    let recipe = crate::crafting::recipe_definition(&job.recipe_id);
    let recipe_name = recipe.map(|r| r.name).unwrap_or("Unknown recipe");
    let display_name = if job.quantity > 1 {
        format!("{recipe_name} ×{}", job.quantity)
    } else {
        recipe_name.to_owned()
    };
    let tint = recipe
        .and_then(|r| item_definition(r.output_item))
        .map(|definition| definition.tint)
        .unwrap_or(ItemTint::new(146, 158, 171));

    let row_center_y = rect.top() + 18.0;

    let dot_radius = 6.0;
    painter.circle_filled(
        Pos2::new(rect.left() + 16.0, row_center_y),
        dot_radius,
        Color32::from_rgb(tint.r, tint.g, tint.b),
    );

    painter.text(
        Pos2::new(rect.left() + 32.0, row_center_y),
        Align2::LEFT_CENTER,
        &display_name,
        FontId::new(13.5, FontFamily::Proportional),
        theme::text(),
    );

    let status_text = if is_head {
        format!("Crafting… {:>3.0}%", fraction * 100.0)
    } else {
        "Queued".to_owned()
    };
    painter.text(
        Pos2::new(
            rect.right() - 12.0 - QUEUE_CANCEL_BUTTON_SIZE - 8.0,
            row_center_y,
        ),
        Align2::RIGHT_CENTER,
        status_text,
        FontId::new(11.5, FontFamily::Proportional),
        theme::muted_text(),
    );

    let bar_height = 6.0;
    let bar_left = rect.left() + 12.0;
    let bar_right = rect.right() - 12.0;
    let bar_top = rect.bottom() - 14.0;
    let bar_bg = Rect::from_min_max(
        Pos2::new(bar_left, bar_top),
        Pos2::new(bar_right, bar_top + bar_height),
    );
    painter.rect_filled(bar_bg, CornerRadius::same(3), theme::input_fill());
    let _ = is_head;
    let fill_right = bar_left + (bar_right - bar_left) * fraction;
    if fill_right > bar_left {
        let bar_fill = Rect::from_min_max(
            Pos2::new(bar_left, bar_top),
            Pos2::new(fill_right, bar_top + bar_height),
        );
        painter.rect_filled(bar_fill, CornerRadius::same(3), theme::accent());
    }

    let cancel_rect = Rect::from_center_size(
        Pos2::new(
            rect.right() - 12.0 - QUEUE_CANCEL_BUTTON_SIZE * 0.5,
            row_center_y,
        ),
        Vec2::splat(QUEUE_CANCEL_BUTTON_SIZE),
    );
    let cancel_response = ui.interact(
        cancel_rect,
        ui.id().with(("crafting_cancel", job.job_id)),
        Sense::click(),
    );
    let hovered = cancel_response.hovered();
    painter.rect_filled(
        cancel_rect,
        CornerRadius::same(4),
        if hovered {
            theme::button_hover_fill()
        } else {
            theme::button_fill()
        },
    );
    painter.rect_stroke(
        cancel_rect,
        CornerRadius::same(4),
        Stroke::new(1.0, theme::button_stroke()),
        StrokeKind::Inside,
    );
    painter.text(
        cancel_rect.center(),
        Align2::CENTER_CENTER,
        "×",
        FontId::new(15.0, FontFamily::Proportional),
        theme::text(),
    );
    theme::record_click_sound(ui, &cancel_response);

    QueueCardResponse {
        cancel_clicked: cancel_response.clicked(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::state::{ClientRuntime, LocalPlayerState},
        crafting::PLANT_TWINE_RECIPE_ID,
        protocol::PlayerCraftingState,
        server::PlayerPrivate,
    };

    fn job(job_id: u64, progress: u32, total: u32) -> CraftingJob {
        let mut j = CraftingJob::new(job_id, PLANT_TWINE_RECIPE_ID, total, 1);
        j.progress_ticks = progress;
        j
    }

    fn local_player_with_jobs(jobs: Vec<CraftingJob>) -> LocalPlayerState {
        LocalPlayerState {
            entity: None,
            private: Some(PlayerPrivate {
                inventory: crate::protocol::PlayerInventoryState::empty(),
                crafting: PlayerCraftingState { jobs },
                open_furnace: None,
                open_loot_bag: None,
                last_processed_input: 0,
                applied_action_seq: 0,
            }),
            lifecycle: None,
        }
    }

    fn run_ui(f: impl FnMut(&egui::Context)) -> egui::FullOutput {
        let ctx = egui::Context::default();
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 768.0),
                )),
                ..Default::default()
            },
            f,
        )
    }

    #[test]
    fn queue_card_alpha_shows_first_three_only() {
        // Top three render at full opacity, anything deeper is handled
        // by the "+N more" overflow bar drawn beneath them.
        assert!((queue_card_alpha(0) - 1.0).abs() < f32::EPSILON);
        assert!((queue_card_alpha(1) - 1.0).abs() < f32::EPSILON);
        assert!((queue_card_alpha(2) - 1.0).abs() < f32::EPSILON);
        assert!(queue_card_alpha(3) <= 0.0);
        assert!(queue_card_alpha(15) <= 0.0);
    }

    #[test]
    fn baseline_from_captures_job_progress_and_clock() {
        let j = job(1, 7, 40);
        let baseline = baseline_from(&j, 12.5);
        assert_eq!(baseline.observed_ticks, 7);
        assert_eq!(baseline.total_ticks, 40);
        assert_eq!(baseline.observed_at_secs, 12.5);
    }

    #[test]
    fn smoothed_fraction_queued_jobs_render_at_zero() {
        let mut hud = CraftingHudState::default();
        let j = job(2, 20, 40);
        // Non-head jobs always show 0, the server doesn't advance them.
        let fraction = smoothed_fraction(&mut hud, &j, 100.0, false);
        assert_eq!(fraction, 0.0);
        // A baseline is still recorded so it can become the head later.
        assert!(hud.progress.contains_key(&j.job_id));
    }

    #[test]
    fn smoothed_fraction_head_anchors_and_advances_with_clock() {
        let mut hud = CraftingHudState::default();
        // Head job halfway through 40 ticks observed at t=0.
        let j = job(3, 20, 40);
        let at_anchor = smoothed_fraction(&mut hud, &j, 0.0, true);
        assert!((at_anchor - 0.5).abs() < 1e-3);

        // Same snapshot a second later: the bar advances off the wall
        // clock at the server tick rate, so the fraction grows.
        let later = smoothed_fraction(&mut hud, &j, 1.0, true);
        assert!(later > at_anchor);
        assert!(later <= 1.0);
    }

    #[test]
    fn smoothed_fraction_zero_total_is_full() {
        let mut hud = CraftingHudState::default();
        let j = job(4, 0, 0);
        assert_eq!(smoothed_fraction(&mut hud, &j, 5.0, true), 1.0);
    }

    #[test]
    fn smoothed_fraction_clamps_at_one() {
        let mut hud = CraftingHudState::default();
        // Already complete, observed long ago → clamped to 1.0.
        let j = job(5, 40, 40);
        let fraction = smoothed_fraction(&mut hud, &j, 1000.0, true);
        assert_eq!(fraction, 1.0);
    }

    #[test]
    fn smoothed_fraction_rebases_when_server_advances() {
        let mut hud = CraftingHudState::default();
        let first = job(6, 10, 40);
        smoothed_fraction(&mut hud, &first, 0.0, true);
        // A new snapshot with more progress rebases the anchor's ticks.
        let second = job(6, 30, 40);
        smoothed_fraction(&mut hud, &second, 0.0, true);
        let baseline = hud.progress.get(&second.job_id).expect("baseline");
        assert_eq!(baseline.observed_ticks, 30);
    }

    #[test]
    fn hud_clears_progress_when_no_jobs() {
        let mut hud = CraftingHudState::default();
        hud.progress.insert(99, baseline_from(&job(99, 1, 2), 0.0));
        let mut runtime = ClientRuntime::default();
        let local = local_player_with_jobs(Vec::new());
        let mut toasts: Vec<String> = Vec::new();

        run_ui(|ctx| {
            crafting_queue_hud(ctx, &mut runtime, &local, &mut hud, &mut toasts);
        });
        // Empty queue forgets every baseline.
        assert!(hud.progress.is_empty());
    }

    #[test]
    fn hud_renders_cards_and_overflow_bar() {
        let mut hud = CraftingHudState::default();
        let jobs = (0..5).map(|i| job(i, 0, 40)).collect();
        let mut runtime = ClientRuntime::default();
        let local = local_player_with_jobs(jobs);
        let mut toasts: Vec<String> = Vec::new();

        let output = run_ui(|ctx| {
            crafting_queue_hud(ctx, &mut runtime, &local, &mut hud, &mut toasts);
        });
        // Five jobs → three cards + a "+2 more" overflow bar all paint.
        assert!(!output.shapes.is_empty());
        // Baselines are only recorded for the visible cards (cards past the
        // window have zero alpha and never call `smoothed_fraction`).
        assert_eq!(hud.progress.len(), QUEUE_VISIBLE_CARDS);
    }

    #[test]
    fn hud_drops_baselines_for_jobs_that_left_queue() {
        let mut hud = CraftingHudState::default();
        // Seed a stale baseline for a job that's no longer present.
        hud.progress
            .insert(404, baseline_from(&job(404, 1, 2), 0.0));
        let mut runtime = ClientRuntime::default();
        let local = local_player_with_jobs(vec![job(0, 0, 40)]);
        let mut toasts: Vec<String> = Vec::new();

        run_ui(|ctx| {
            crafting_queue_hud(ctx, &mut runtime, &local, &mut hud, &mut toasts);
        });
        assert!(!hud.progress.contains_key(&404));
        assert!(hud.progress.contains_key(&0));
    }
}
