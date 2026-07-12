use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy_egui::egui;

use crate::{
    app::{
        state::{
            ClientRuntime, ClientSettings, CombatFeedbackState, LocalPlayerState, RangedDrawState,
            ThrowChargeState,
        },
        voice::VoiceState,
    },
    inventory::count_items_in_inventory,
    items::item_definition,
    protocol::MAX_HEALTH,
};

use super::theme;

const HEALTH_WIDTH: f32 = 192.0;
const HEALTH_HEIGHT: f32 = 30.0;
const HEALTH_ICON_WIDTH: f32 = 30.0;
/// Height of the draw/reload progress bar that sits just above the actionbar, in
/// px. A slim status readout, not a focal element, but tall enough to read at a
/// glance from across the screen (the earlier 5 px bar was too thin to see).
const RANGED_BAR_HEIGHT: f32 = 8.0;
/// Vertical gap between the top of the actionbar and the bottom of the ranged
/// progress bar, in px, so the bar floats clear of the actionbar frame.
const RANGED_BAR_GAP: f32 = 6.0;
/// Inset of the ammo count from the actionbar's lower-right corner, in px. Places
/// the small count inside the bottom-right of the actionbar frame where it reads
/// at a glance without covering a slot.
const RANGED_AMMO_INSET: egui::Vec2 = egui::vec2(6.0, 4.0);
/// Fixed width of the perf overlay so chunk labels like
/// `(-99, -99) Rocky outcrop` don't push the values column around.
const PERF_BOX_WIDTH: f32 = 240.0;
const PERF_LABEL_WIDTH: f32 = 96.0;
const PERF_VALUE_WIDTH: f32 = 124.0;

#[allow(clippy::too_many_arguments)]
pub(super) fn hud_ui(
    ctx: &egui::Context,
    runtime: &ClientRuntime,
    diagnostics: &DiagnosticsStore,
    settings: &ClientSettings,
    voice: &VoiceState,
    combat: &CombatFeedbackState,
    local_player: &LocalPlayerState,
    ranged: &RangedDrawState,
    throw_charge: &ThrowChargeState,
    actionbar_rect: Option<egui::Rect>,
) {
    if settings.hud.show_perf_stats {
        perf_stats_ui(ctx, runtime, diagnostics);
    }
    if runtime.connection_is_lagging() {
        connection_lag_indicator(ctx);
    }

    voice_indicator(ctx, voice);
    meteor_shower_hud(ctx, runtime);

    let Some(player) = runtime.local_view() else {
        return;
    };

    low_health_vignette(ctx, player.health);
    combat_feedback_ui(
        ctx,
        combat,
        player.position.x,
        player.position.z,
        player.yaw,
    );
    if let Some(view) = ranged_hud_view(local_player, ranged) {
        ranged_hud(ctx, &view, actionbar_rect);
    } else if let Some(view) = throw_hud_view(local_player, throw_charge) {
        // The bomb's charge reuses the ranged bar wholesale (same track, same
        // brightening draw fill), with the held bomb count as the "ammo".
        ranged_hud(ctx, &view, actionbar_rect);
    }

    egui::Area::new("hud_bars".into())
        .anchor(egui::Align2::RIGHT_BOTTOM, [-18.0, -18.0])
        .show(ctx, |ui| {
            health_bar(ui, player.health);
        });
}

/// What the ranged progress bar is filling with: a bow draw ramping to full, or a
/// crossbow reload cranking back to ready.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RangedHudFill {
    Draw(f32),
    Reload(f32),
}

/// Resolved per-frame inputs for the ranged HUD: the arrow count and (while a draw
/// or reload is live) the progress-bar fill. `None` whenever the active item is
/// not a ranged weapon, which is what keeps the HUD silent for melee / tools /
/// bare hands.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RangedHudView {
    ammo: u32,
    fill: Option<RangedHudFill>,
}

/// Resolve the ranged HUD view off the active item + draw state. `None` unless a
/// ranged weapon is the active actionbar item.
fn ranged_hud_view(
    local_player: &LocalPlayerState,
    ranged: &RangedDrawState,
) -> Option<RangedHudView> {
    let private = local_player.private.as_ref()?;
    let profile = private
        .inventory
        .active_actionbar_stack()
        .and_then(|stack| item_definition(&stack.item_id))
        .and_then(|definition| definition.ranged)?;
    let ammo = count_items_in_inventory(&private.inventory, profile.ammo_item);
    let fill = if ranged.is_drawing() {
        Some(RangedHudFill::Draw(ranged.draw_fraction()))
    } else if ranged.is_reloading() {
        Some(RangedHudFill::Reload(ranged.reload_fraction()))
    } else {
        None
    };
    Some(RangedHudView { ammo, fill })
}

/// Resolve the thrown-bomb charge HUD view: the held bomb count plus (while a
/// charge is held) the ranged draw bar filling with the charge fraction. `None`
/// unless the active item is a thrown explosive AND a charge is live, so the
/// bar stays silent while just carrying a bomb.
fn throw_hud_view(
    local_player: &LocalPlayerState,
    throw_charge: &ThrowChargeState,
) -> Option<RangedHudView> {
    if !throw_charge.is_charging() {
        return None;
    }
    let private = local_player.private.as_ref()?;
    let stack = private.inventory.active_actionbar_stack()?;
    let explosive = item_definition(&stack.item_id).and_then(|def| def.explosive)?;
    if explosive.delivery != crate::items::ExplosiveDelivery::Thrown {
        return None;
    }
    Some(RangedHudView {
        ammo: count_items_in_inventory(&private.inventory, &stack.item_id),
        fill: Some(RangedHudFill::Draw(throw_charge.charge_fraction())),
    })
}

/// The actionbar-anchored ranged readout: a small ammo count tucked into the
/// bottom-right of the actionbar, and (only while drawing / reloading) a
/// semi-transparent horizontal progress bar sitting just above the actionbar that
/// fills left-to-right with the draw fraction (bow) or reload progress (crossbow).
/// Both anchor off `actionbar_rect` (one frame stale, which is fine). Painted on a
/// dedicated foreground layer like the hit marker so no Area clips it. When the
/// actionbar rect isn't known yet (pre-first-frame), the HUD stays silent rather
/// than falling back to the crosshair.
fn ranged_hud(ctx: &egui::Context, view: &RangedHudView, actionbar_rect: Option<egui::Rect>) {
    let Some(bar) = actionbar_rect else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("ranged_hud"),
    ));

    // Ammo count: small, monospace, muted; warms to red when the quiver is empty
    // so "why won't it fire" answers itself. Anchored to the actionbar's
    // bottom-right corner, inset so it reads as a corner label.
    let ammo_color = if view.ammo == 0 {
        egui::Color32::from_rgba_unmultiplied(235, 110, 90, 235)
    } else {
        egui::Color32::from_rgba_unmultiplied(230, 233, 238, 200)
    };
    painter.text(
        bar.right_bottom() - RANGED_AMMO_INSET,
        egui::Align2::RIGHT_BOTTOM,
        format!("{}", view.ammo),
        egui::FontId::monospace(14.0),
        ammo_color,
    );

    // Draw / reload progress bar: a translucent track plus the filled sweep,
    // spanning the actionbar width and sitting just above it. Only painted while a
    // draw or reload is live, so it is quiet during idle aim.
    let Some(fill) = view.fill else {
        return;
    };
    let (fraction, color) = match fill {
        // Draw charge: brightens as it fills so full draw reads at a glance. Warm
        // bright cord that goes near-opaque at full draw.
        RangedHudFill::Draw(f) => {
            let f = f.clamp(0.0, 1.0);
            let alpha = (170.0 + 80.0 * f) as u8;
            (
                f,
                egui::Color32::from_rgba_unmultiplied(240, 242, 245, alpha),
            )
        }
        // Reload progress: a cool "busy" fill, but bright enough to read clearly
        // against a lit world (the earlier dim value washed out on close inspection).
        RangedHudFill::Reload(f) => (
            f.clamp(0.0, 1.0),
            egui::Color32::from_rgba_unmultiplied(150, 200, 255, 230),
        ),
    };
    let (track_rect, fill_rect) = ranged_bar_rects(bar, fraction);
    // A darker opaque backing under the whole track so the fill reads with real
    // contrast against any world colour behind it, not just a faint tint.
    painter.rect_filled(
        track_rect,
        2.0,
        egui::Color32::from_rgba_unmultiplied(12, 16, 22, 200),
    );
    if fill_rect.width() > 0.0 {
        painter.rect_filled(fill_rect, 2.0, color);
    }
    // A thin light frame around the track so its extent is legible even at a low
    // fill fraction.
    painter.rect_stroke(
        track_rect,
        2.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(230, 233, 238, 90),
        ),
        egui::StrokeKind::Inside,
    );
    // Keep the bar animating while it is live.
    ctx.request_repaint();
}

/// Geometry for the ranged progress bar: the full-width translucent track and the
/// left-to-right fill, both sitting `RANGED_BAR_GAP` above `actionbar` and
/// spanning its width. Pure so the placement is unit-testable without egui state.
fn ranged_bar_rects(actionbar: egui::Rect, fraction: f32) -> (egui::Rect, egui::Rect) {
    let fraction = fraction.clamp(0.0, 1.0);
    let bottom = actionbar.top() - RANGED_BAR_GAP;
    let top = bottom - RANGED_BAR_HEIGHT;
    let track = egui::Rect::from_min_max(
        egui::pos2(actionbar.left(), top),
        egui::pos2(actionbar.right(), bottom),
    );
    let fill = egui::Rect::from_min_max(
        track.min,
        egui::pos2(track.left() + track.width() * fraction, track.bottom()),
    );
    (track, fill)
}

/// Subtle "transmitting" chip anchored under the top-center of the screen.
/// Stays invisible while idle, eases in when push-to-talk is held, eases
/// back out a beat after release. Also pulses gently to make it feel alive
/// without being distracting.
fn voice_indicator(ctx: &egui::Context, voice: &VoiceState) {
    if voice.indicator_envelope <= 0.005 {
        return;
    }
    let envelope = voice.indicator_envelope.clamp(0.0, 1.0);
    let alpha_byte = (envelope * 230.0) as u8;
    if alpha_byte == 0 {
        return;
    }

    // Gentle 0.8 Hz pulse on the mic-glyph color so the indicator reads as
    // "live" rather than a static label.
    let time = ctx.input(|input| input.time);
    let pulse = 0.5 + 0.5 * ((time as f32) * std::f32::consts::TAU * 0.8).sin();
    let pulse_alpha = (140.0 + pulse * 80.0) as u8;
    let glyph_color = egui::Color32::from_rgba_unmultiplied(248, 128, 96, pulse_alpha);

    let slide_in = (1.0 - envelope) * 12.0;

    egui::Area::new("voice_indicator".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 24.0 + slide_in])
        .show(ctx, |ui| {
            ui.set_opacity(envelope);
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(6, 9, 13, alpha_byte))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(248, 128, 96, 96),
                ))
                .corner_radius(12)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Painter-drawn pulse dot. Using a text glyph like
                        // "●" rendered as a red tofu box on this machine
                        // because egui's bundled font doesn't include
                        // U+25CF, a hand-drawn circle has no font dep.
                        let dot_radius = 4.5;
                        let (dot_rect, _) = ui.allocate_exact_size(
                            egui::vec2(dot_radius * 2.0, dot_radius * 2.0),
                            egui::Sense::hover(),
                        );
                        ui.painter()
                            .circle_filled(dot_rect.center(), dot_radius, glyph_color);
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new("Voice On")
                                .size(12.5)
                                .strong()
                                .color(theme::text()),
                        );
                    });
                });
        });
    if envelope < 0.999 {
        ctx.request_repaint();
    }
}

/// Format a meteor shower countdown for the HUD pill from the seconds remaining to
/// impact. Under 30 s it switches to a terse "Impact imminent"; otherwise it
/// reads `M:SS`. Pure so it can be unit-tested.
fn format_meteor_shower_countdown(seconds_to_impact: f32) -> String {
    if seconds_to_impact <= 30.0 {
        return "Impact imminent".to_owned();
    }
    let total = seconds_to_impact.max(0.0) as u32;
    let minutes = total / 60;
    let secs = total % 60;
    format!("MeteorShower {minutes}:{secs:02}")
}

/// Escalation intensity `0.0..=1.0` for the danger-zone evacuation warning,
/// ramping over the final 60 seconds before impact. `0.0` at 60 s out, `1.0` at
/// impact, held at `1.0` briefly after. Pure so the threshold is unit-testable.
fn meteor_shower_danger_intensity(seconds_to_impact: f32) -> f32 {
    // Before the last minute the warning is present but calm (a small floor);
    // in the final 60 s it climbs to full.
    ((60.0 - seconds_to_impact) / 60.0).clamp(0.0, 1.0)
}

/// The meteor shower countdown pill and the danger-zone evacuation warning. Both are
/// computed client-side from the announce payload (`runtime.meteor_shower`) plus the
/// player's own position against the authoritative clock estimate, so they cost
/// nothing on the wire and can never desync from the countdown. Silent when no
/// event is live or after impact.
fn meteor_shower_hud(ctx: &egui::Context, runtime: &ClientRuntime) {
    let Some(event) = runtime.meteor_shower else {
        return;
    };
    let now = runtime.server_tick();
    // Only the pre-impact fireball phase gets a countdown; after impact the pill
    // and warning go quiet (the crater visual takes over).
    if event.has_impacted(now) {
        return;
    }
    let seconds_to_impact = event.seconds_to_impact(now);

    // Countdown pill: CENTER_TOP, the voice-indicator template.
    let imminent = seconds_to_impact <= 30.0;
    let (fill, stroke, text_color) = if imminent {
        (
            egui::Color32::from_rgba_unmultiplied(38, 10, 6, 235),
            egui::Color32::from_rgba_unmultiplied(255, 120, 70, 200),
            egui::Color32::from_rgb(255, 190, 150),
        )
    } else {
        (
            egui::Color32::from_rgba_unmultiplied(10, 8, 12, 220),
            egui::Color32::from_rgba_unmultiplied(248, 150, 70, 110),
            theme::text(),
        )
    };
    let label = format_meteor_shower_countdown(seconds_to_impact);
    egui::Area::new("meteor_shower_countdown".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 24.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, stroke))
                .corner_radius(12)
                .inner_margin(egui::Margin::symmetric(14, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // A small painter-drawn ember dot, warmer/pulsing when
                        // imminent (no font glyph dependency, same reason as the
                        // voice indicator).
                        let dot_radius = 4.5;
                        let (dot_rect, _) = ui.allocate_exact_size(
                            egui::vec2(dot_radius * 2.0, dot_radius * 2.0),
                            egui::Sense::hover(),
                        );
                        let pulse = if imminent {
                            let t = ctx.input(|input| input.time) as f32;
                            0.5 + 0.5 * (t * std::f32::consts::TAU * 1.6).sin()
                        } else {
                            1.0
                        };
                        let dot_alpha = (150.0 + pulse * 105.0) as u8;
                        ui.painter().circle_filled(
                            dot_rect.center(),
                            dot_radius,
                            egui::Color32::from_rgba_unmultiplied(255, 120, 60, dot_alpha),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new(label)
                                .size(13.0)
                                .strong()
                                .color(text_color),
                        );
                    });
                });
        });
    if imminent {
        ctx.request_repaint();
    }

    // Danger-zone evacuation warning: only when the player's OWN position is
    // inside the danger radius of the impact point. Escalates over the final 60 s
    // (colour + pulse). Because the impact siting guarantees the clearance
    // exceeds the blast, the warning directs players OUT of the zone rather than
    // under a roof that cannot exist inside it.
    let Some(player) = runtime.local_view() else {
        return;
    };
    let inside_danger = event.impact_position.within_horizontal_range(
        player.position,
        crate::game_balance::METEOR_SHOWER_DANGER_RADIUS_M,
    );
    if !inside_danger {
        return;
    }
    let intensity = meteor_shower_danger_intensity(seconds_to_impact);
    meteor_shower_danger_warning(ctx, intensity);
}

/// The escalating "evacuate the area" banner shown while the player stands inside
/// the danger zone. Sits below the countdown pill; its colour saturates and it
/// pulses faster as `intensity` (0..=1, ramping over the final 60 s) climbs.
fn meteor_shower_danger_warning(ctx: &egui::Context, intensity: f32) {
    let intensity = intensity.clamp(0.0, 1.0);
    let t = ctx.input(|input| input.time) as f32;
    // Pulse frequency rises with intensity so it feels more urgent late.
    let freq = 1.0 + intensity * 3.0;
    let pulse = 0.5 + 0.5 * (t * std::f32::consts::TAU * freq).sin();
    // Border/background redden and brighten with intensity + pulse.
    let base = 60.0 + intensity * 140.0;
    let glow = (base + pulse * 55.0).min(255.0) as u8;
    let fill_alpha = (150.0 + intensity * 90.0) as u8;

    egui::Area::new("meteor_shower_danger".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 64.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(48, 6, 4, fill_alpha))
                .stroke(egui::Stroke::new(
                    1.5 + intensity * 1.5,
                    egui::Color32::from_rgba_unmultiplied(glow, 40, 24, 235),
                ))
                .corner_radius(10)
                .inner_margin(egui::Margin::symmetric(16, 8))
                .show(ui, |ui| {
                    let green = 200u8.saturating_sub((intensity * 80.0) as u8);
                    // Extend, never wrap: the banner must stay one line however
                    // large the late-intensity text grows.
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("Meteor shower incoming. Evacuate the area.")
                                .size(15.0 + intensity * 3.0)
                                .strong()
                                .color(egui::Color32::from_rgb(255, green, 170)),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
        });
    // Always animating while the warning is up.
    ctx.request_repaint();
}

/// Small chip rendered in the top-left when the session has gone silent
/// long enough to be suspicious. Stays out of the way during normal play
/// but is immediately visible the moment the link goes wobbly.
fn connection_lag_indicator(ctx: &egui::Context) {
    egui::Area::new("connection_lag".into())
        .anchor(egui::Align2::LEFT_TOP, [16.0, 14.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(58, 24, 16, 220))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(220, 120, 80, 200),
                ))
                .corner_radius(5)
                .inner_margin(egui::Margin::symmetric(10, 5))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("Connection unstable")
                            .size(12.5)
                            .color(egui::Color32::from_rgb(252, 224, 196)),
                    );
                });
        });
}

/// Frame-pacing snapshot drawn by the perf overlay. The smoothed FPS that
/// Bevy exposes by default hides periodic stalls, a stream of
/// `[2 ms, 2 ms, 2 ms, 30 ms]` reads as "~120 FPS" but feels like a 30 ms
/// hitch every fourth frame. `p99_ms` and `max_ms` are the actual signal
/// for "the game *feels* slow".
struct FrameTimeStats {
    fps: f64,
    last_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

fn frame_time_stats(diagnostics: &DiagnosticsStore) -> FrameTimeStats {
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|diagnostic| diagnostic.smoothed())
        .unwrap_or(0.0);

    let frame_time = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME);
    let last_ms = frame_time.and_then(|d| d.value()).unwrap_or(0.0);

    let mut samples: Vec<f64> = frame_time
        .map(|d| d.values().copied().collect())
        .unwrap_or_default();

    let (p99_ms, max_ms) = if samples.is_empty() {
        (0.0, 0.0)
    } else {
        // Partial sort is overkill for ~480 samples; full sort is fast
        // enough and the path only runs while the overlay is visible.
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = samples.len();
        let p99_index = (((n as f64) * 0.99) as usize).min(n - 1);
        (samples[p99_index], *samples.last().unwrap_or(&0.0))
    };

    FrameTimeStats {
        fps,
        last_ms,
        p99_ms,
        max_ms,
    }
}

fn perf_stats_ui(ctx: &egui::Context, runtime: &ClientRuntime, diagnostics: &DiagnosticsStore) {
    let frame = frame_time_stats(diagnostics);

    egui::Area::new("perf_stats".into())
        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 14.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(6, 9, 13, 170))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(115, 132, 151, 60),
                ))
                .corner_radius(5)
                .inner_margin(egui::Margin::symmetric(10, 7))
                .show(ui, |ui| {
                    ui.set_width(PERF_BOX_WIDTH);
                    // `add_sized` centers the inner widget within its cell,
                    // which left short values floating mid-column. Pin both
                    // cells to a left-to-right inner layout so each text
                    // anchors flush against the left edge of its cell.
                    let label = |ui: &mut egui::Ui, name: &str, value: String| {
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(PERF_LABEL_WIDTH, 14.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(name)
                                            .size(11.5)
                                            .color(theme::muted_text())
                                            .monospace(),
                                    );
                                },
                            );
                            ui.allocate_ui_with_layout(
                                egui::vec2(PERF_VALUE_WIDTH, 14.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(value)
                                            .size(11.5)
                                            .color(theme::text())
                                            .monospace(),
                                    );
                                },
                            );
                        });
                    };
                    ui.label(
                        egui::RichText::new("Performance")
                            .size(11.5)
                            .strong()
                            .color(theme::text()),
                    );
                    ui.add_space(2.0);
                    label(ui, "FPS", format!("{:.0}", frame.fps));
                    // Frame-time triplet exposes the hitches the smoothed
                    // FPS number hides. `now` is the most recent frame in
                    // ms; `p99` and `max` are computed over the diagnostic
                    // history window (~1 s at 500 FPS). If p99/max diverge
                    // from `now` you are seeing periodic stalls even when
                    // the FPS readout looks healthy.
                    label(ui, "Frame", format!("{:.2} ms", frame.last_ms));
                    label(ui, "p99 frame", format!("{:.2} ms", frame.p99_ms));
                    label(ui, "max frame", format!("{:.2} ms", frame.max_ms));
                    match runtime.perf_stats {
                        Some(stats) => {
                            label(
                                ui,
                                "Chunk",
                                format!(
                                    "({}, {}) {}",
                                    stats.player_chunk_x,
                                    stats.player_chunk_z,
                                    stats.player_classification.label()
                                ),
                            );
                            label(ui, "Loaded", stats.loaded_chunks.to_string());
                            label(ui, "Live nodes", stats.live_nodes.to_string());
                            label(ui, "Visible", stats.aoi_visible_nodes.to_string());
                            label(ui, "Regrow queue", stats.pending_regrows.to_string());
                        }
                        None => {
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new("waiting for server…")
                                    .size(11.0)
                                    .italics()
                                    .color(theme::muted_text()),
                            );
                        }
                    }
                });
        });
}

/// Fraction of max health at/under which the low-health vignette begins to
/// fade in. Above this the screen edges stay clean.
const VIGNETTE_THRESHOLD: f32 = 0.30;
/// Peak edge alpha at zero health. Moderate on purpose: the effect should read
/// as "you're badly hurt" without blacking out the periphery.
const VIGNETTE_PEAK_ALPHA: f32 = 150.0;

/// Red screen-edge vignette that fades in as the local player's health drops
/// below [`VIGNETTE_THRESHOLD`]. Drawn as a single gradient mesh, opaque at the
/// screen border and transparent toward the center, so it darkens the
/// periphery without covering the crosshair area. Pulses gently once health is
/// critical to nudge the player toward retreating or healing.
fn low_health_vignette(ctx: &egui::Context, health: f32) {
    let fraction = (health / MAX_HEALTH).clamp(0.0, 1.0);
    if fraction >= VIGNETTE_THRESHOLD {
        return;
    }
    // 0 at the threshold, 1 at empty.
    let severity = ((VIGNETTE_THRESHOLD - fraction) / VIGNETTE_THRESHOLD).clamp(0.0, 1.0);

    // Gentle 1.5 Hz pulse, scaled in only as health approaches zero so a
    // low-but-stable bar doesn't throb forever.
    let time = ctx.input(|input| input.time) as f32;
    let wave = 0.85 + 0.15 * (time * std::f32::consts::TAU * 1.5).sin();
    let critical = (1.0 - fraction / VIGNETTE_THRESHOLD).clamp(0.0, 1.0);
    let pulse = 1.0 + (wave - 1.0) * critical;

    let peak_alpha = (severity * VIGNETTE_PEAK_ALPHA * pulse).clamp(0.0, 255.0) as u8;
    if peak_alpha == 0 {
        return;
    }

    // A background-layer painter spans the whole screen, so the gradient mesh
    // isn't clipped to an auto-sized Area (which collapses to nothing since we
    // only paint and never allocate widgets).
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("low_health_vignette"),
    ));
    let edge = egui::Color32::from_rgba_unmultiplied(120, 16, 16, peak_alpha);
    let center = egui::Color32::from_rgba_unmultiplied(120, 16, 16, 0);
    // Inset to where the red fully fades out. ~22% of the smaller dimension
    // keeps the clear center generous on any aspect ratio (and well under half,
    // so the inner rect can never invert).
    let inset = screen.size().min_elem() * 0.22;
    let inner = screen.shrink(inset);
    painter.add(vignette_mesh(screen, inner, edge, center));

    // Keep the pulse animating while the vignette is visible.
    ctx.request_repaint();
}

/// On-crosshair hit marker plus the damage-direction arrows. Both are
/// screen-space overlays painted on dedicated full-screen layers so they read
/// against the world without an Area clipping them. `px`/`pz` are the local
/// player's horizontal position and `yaw` their look heading, used to place the
/// arrows relative to where the player is facing.
fn combat_feedback_ui(
    ctx: &egui::Context,
    combat: &CombatFeedbackState,
    px: f32,
    pz: f32,
    yaw: f32,
) {
    hit_marker(ctx, combat.hit_marker_fade(), combat.hit_marker_is_player());
    damage_direction(ctx, combat, px, pz, yaw);

    // Animate the fades while anything is on-screen.
    if combat.is_active() {
        ctx.request_repaint();
    }
}

/// Classic four-tick "X" hit marker at screen center that fades and expands
/// slightly as it decays. White for world hits, hot red for PvP connects.
fn hit_marker(ctx: &egui::Context, fade: f32, is_player: bool) {
    if fade <= 0.0 {
        return;
    }
    let center = ctx.content_rect().center();
    // Grow the gap as it fades so the marker reads as a quick "pop".
    let grow = 1.0 + (1.0 - fade) * 0.6;
    let gap = 4.0 * grow;
    let len = 7.0;
    let alpha = (fade * 235.0) as u8;
    let color = if is_player {
        egui::Color32::from_rgba_unmultiplied(255, 90, 78, alpha)
    } else {
        egui::Color32::from_rgba_unmultiplied(240, 242, 245, alpha)
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("hit_marker"),
    ));
    let stroke = egui::Stroke::new(2.0, color);
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let inner = egui::pos2(center.x + sx * gap, center.y + sy * gap);
        let outer = egui::pos2(center.x + sx * (gap + len), center.y + sy * (gap + len));
        painter.line_segment([inner, outer], stroke);
    }
}

/// Red chevrons on a ring around the crosshair, one per recent hit, each
/// pointing toward the attacker's bearing relative to the player's facing.
/// Fades with each arrow's remaining lifetime.
fn damage_direction(ctx: &egui::Context, combat: &CombatFeedbackState, px: f32, pz: f32, yaw: f32) {
    let arrows = combat.damage_arrows();
    if arrows.is_empty() {
        return;
    }
    let rect = ctx.content_rect();
    let center = rect.center();
    let radius = rect.size().min_elem() * 0.18;

    // Horizontal forward + right basis from the look heading. `look_forward`
    // owns the yaw convention; `right = forward x up` lines up with egui's
    // screen axes (x right, y down) so a hit from world-right shows on the
    // right of the ring.
    let forward = crate::items::look_forward(yaw, 0.0);
    let flen = (forward.x * forward.x + forward.z * forward.z)
        .sqrt()
        .max(1e-4);
    let (fx, fz) = (forward.x / flen, forward.z / flen);

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("damage_direction"),
    ));
    for arrow in arrows {
        let dx = arrow.source.x - px;
        let dz = arrow.source.z - pz;
        if dx * dx + dz * dz < 1e-4 {
            continue;
        }
        let ahead = dx * fx + dz * fz;
        let side = dx * (-fz) + dz * fx;
        // angle 0 = directly ahead -> top of the ring; +angle (world-right) ->
        // right side of the ring.
        let angle = side.atan2(ahead);
        let dir = egui::vec2(angle.sin(), -angle.cos());
        let tangent = egui::vec2(-dir.y, dir.x);
        let pos = center + dir * radius;
        let tip = pos + dir * 13.0;
        let base_a = pos - dir * 4.0 + tangent * 9.0;
        let base_b = pos - dir * 4.0 - tangent * 9.0;
        let alpha = (arrow.fade() * 220.0) as u8;
        let fill = egui::Color32::from_rgba_unmultiplied(214, 40, 40, alpha);
        painter.add(egui::Shape::convex_polygon(
            vec![tip, base_a, base_b],
            fill,
            egui::Stroke::NONE,
        ));
    }
}

/// Build the four-quad "frame" mesh between `outer` (filled with `edge`) and
/// `inner` (filled with `center`). Each side fades from the screen border
/// inward; the shared corner vertices keep the corners strongest, which is the
/// classic vignette falloff.
fn vignette_mesh(
    outer: egui::Rect,
    inner: egui::Rect,
    edge: egui::Color32,
    center: egui::Color32,
) -> egui::Shape {
    use egui::epaint::{Vertex, WHITE_UV};
    let mut mesh = egui::Mesh::default();

    // 0..=3 outer corners (edge color), 4..=7 matching inner corners (center
    // color), both in TL, TR, BR, BL order.
    let corners = [
        (outer.left_top(), edge),
        (outer.right_top(), edge),
        (outer.right_bottom(), edge),
        (outer.left_bottom(), edge),
        (inner.left_top(), center),
        (inner.right_top(), center),
        (inner.right_bottom(), center),
        (inner.left_bottom(), center),
    ];
    for (pos, color) in corners {
        mesh.vertices.push(Vertex {
            pos,
            uv: WHITE_UV,
            color,
        });
    }

    // Four border quads (top, right, bottom, left), each split into two
    // triangles. Indices reference the vertex list above.
    let quads: [(u32, u32, u32, u32); 4] = [
        (0, 1, 5, 4), // top
        (1, 2, 6, 5), // right
        (2, 3, 7, 6), // bottom
        (3, 0, 4, 7), // left
    ];
    for (o0, o1, i1, i0) in quads {
        mesh.indices.extend_from_slice(&[o0, o1, i1, o0, i1, i0]);
    }

    egui::Shape::mesh(mesh)
}

fn health_bar(ui: &mut egui::Ui, health: f32) {
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
    use super::*;
    use crate::protocol::{PlayerState, Vec3Net};

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        }
    }

    #[test]
    fn meteor_shower_countdown_formats_minutes_and_switches_to_imminent() {
        // Well out: M:SS with zero-padded seconds.
        assert_eq!(format_meteor_shower_countdown(504.0), "MeteorShower 8:24");
        assert_eq!(format_meteor_shower_countdown(65.0), "MeteorShower 1:05");
        // The 30 s boundary and below switches to the terse imminent copy.
        assert_eq!(format_meteor_shower_countdown(31.0), "MeteorShower 0:31");
        assert_eq!(format_meteor_shower_countdown(30.0), "Impact imminent");
        assert_eq!(format_meteor_shower_countdown(5.0), "Impact imminent");
        assert_eq!(format_meteor_shower_countdown(0.0), "Impact imminent");
    }

    #[test]
    fn meteor_shower_danger_intensity_ramps_over_the_final_minute() {
        // Before the last minute: no escalation yet.
        assert_eq!(meteor_shower_danger_intensity(120.0), 0.0);
        assert_eq!(meteor_shower_danger_intensity(60.0), 0.0);
        // Halfway through the final minute: ~half intensity.
        assert!((meteor_shower_danger_intensity(30.0) - 0.5).abs() < 1e-3);
        // At (and past) impact: full intensity, clamped.
        assert_eq!(meteor_shower_danger_intensity(0.0), 1.0);
        assert_eq!(meteor_shower_danger_intensity(-5.0), 1.0);
        // Monotonic as impact nears.
        assert!(meteor_shower_danger_intensity(10.0) > meteor_shower_danger_intensity(40.0));
    }

    fn player(health: f32) -> PlayerState {
        PlayerState {
            client_id: 1,
            position: Vec3Net::new(1.0, 2.0, 3.0),
            velocity: Vec3Net::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            health,
            grounded: true,
            last_processed_input: 0,
        }
    }

    #[test]
    fn hud_renders_with_and_without_local_player() {
        let ctx = egui::Context::default();
        let diagnostics = DiagnosticsStore::default();
        let mut runtime = ClientRuntime::default();
        let voice = VoiceState::default();

        let _ = ctx.run(raw_input(), |ctx| {
            hud_ui(
                ctx,
                &runtime,
                &diagnostics,
                &ClientSettings::default(),
                &voice,
                &CombatFeedbackState::default(),
                &LocalPlayerState::default(),
                &RangedDrawState::default(),
                &ThrowChargeState::default(),
                None,
            );
        });

        // After Welcome seeds `predicted_local` the HUD has a view to
        // render. Snapshot used to drive this too, but Phase 6.2
        // removed the fallback.
        runtime.client_id = Some(1);
        runtime.predicted_local = Some(crate::controller::PlayerController::from_player_state(
            &player(75.0),
        ));

        let _ = ctx.run(raw_input(), |ctx| {
            hud_ui(
                ctx,
                &runtime,
                &diagnostics,
                &ClientSettings::default(),
                &voice,
                &CombatFeedbackState::default(),
                &LocalPlayerState::default(),
                &RangedDrawState::default(),
                &ThrowChargeState::default(),
                None,
            );
        });

        assert_eq!(runtime.local_view().expect("local player").health, 75.0);
    }

    #[test]
    fn voice_indicator_appears_when_envelope_above_zero() {
        let ctx = egui::Context::default();
        let mut voice = VoiceState::default();
        voice.indicator_envelope = 0.5;

        let output = ctx.run(raw_input(), |ctx| voice_indicator(ctx, &voice));
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn voice_indicator_hidden_when_idle() {
        let ctx = egui::Context::default();
        let voice = VoiceState::default();

        let output = ctx.run(raw_input(), |ctx| voice_indicator(ctx, &voice));
        // No voice activity → nothing drawn.
        assert!(output.shapes.is_empty());
    }

    #[test]
    fn health_bar_clamps_extreme_values() {
        let ctx = egui::Context::default();

        let _ = ctx.run(raw_input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
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
            let output = ctx.run(raw_input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    health_bar(ui, health);
                });
            });
            assert!(!output.shapes.is_empty());
        }
    }

    #[test]
    fn low_health_vignette_appears_only_when_hurt() {
        // Full health: the edges stay clean (nothing drawn).
        let ctx = egui::Context::default();
        let healthy = ctx.run(raw_input(), |ctx| low_health_vignette(ctx, MAX_HEALTH));
        assert!(healthy.shapes.is_empty());

        // Critically low: the gradient mesh is emitted.
        let ctx = egui::Context::default();
        let hurt = ctx.run(raw_input(), |ctx| {
            low_health_vignette(ctx, MAX_HEALTH * 0.1);
        });
        assert!(!hurt.shapes.is_empty());
    }

    #[test]
    fn combat_feedback_overlay_draws_when_active() {
        // Idle state: no hit marker, no arrows, nothing painted.
        let ctx = egui::Context::default();
        let idle = ctx.run(raw_input(), |ctx| {
            combat_feedback_ui(ctx, &CombatFeedbackState::default(), 0.0, 0.0, 0.0);
        });
        assert!(idle.shapes.is_empty());

        // Active marker + a directional arrow both emit shapes.
        let mut combat = CombatFeedbackState::default();
        combat.trigger_hit_marker(true);
        combat.push_damage_from(bevy::math::Vec3::new(5.0, 0.0, 5.0));
        let ctx = egui::Context::default();
        let active = ctx.run(raw_input(), |ctx| {
            combat_feedback_ui(ctx, &combat, 0.0, 0.0, 0.0);
        });
        assert!(!active.shapes.is_empty());
    }

    /// A `LocalPlayerState` whose active actionbar item is `item_id`, with
    /// `arrows` arrows in the bag, for the ranged-HUD resolver tests.
    fn local_player_holding(item_id: &str, arrows: u16) -> LocalPlayerState {
        use crate::protocol::{ItemStack, PlayerInventoryState};
        let mut inventory = PlayerInventoryState::empty();
        inventory.actionbar_slots[0] = Some(ItemStack::new(item_id, 1));
        if arrows > 0 {
            inventory.inventory_slots[0] = Some(ItemStack::new("arrow", arrows));
        }
        LocalPlayerState {
            entity: None,
            private: Some(crate::server::PlayerPrivate {
                inventory,
                crafting: crate::protocol::PlayerCraftingState::default(),
                open_furnace: None,
                open_loot_bag: None,
                open_workbench: None,
                last_processed_input: 0,
                applied_action_seq: 0,
                run_speed_multiplier: 1.0,
            }),
            lifecycle: None,
        }
    }

    #[test]
    fn ranged_hud_is_silent_for_melee_or_empty_hands() {
        // No private yet (pre-connect) and a melee tool both resolve to no view,
        // which is what keeps the HUD dark for everything that isn't a bow.
        let ranged = RangedDrawState::default();
        assert!(ranged_hud_view(&LocalPlayerState::default(), &ranged).is_none());
        assert!(
            ranged_hud_view(&local_player_holding("stone_hatchet", 5), &ranged).is_none(),
            "a melee tool never shows the ranged HUD"
        );
    }

    #[test]
    fn ranged_hud_view_reports_ammo_and_draw_fill() {
        // Holding a bow with 5 arrows: the view carries the count; idle shows no
        // bar fill; a held draw fills the bar with the draw fraction.
        let local = local_player_holding("wooden_bow", 5);
        let mut ranged = RangedDrawState::default();
        let idle = ranged_hud_view(&local, &ranged).expect("a held bow shows the HUD");
        assert_eq!(idle.ammo, 5);
        assert_eq!(idle.fill, None, "no bar fill while not drawing");

        // Start + hold a draw: the fill tracks the draw fraction.
        let profile = crate::items::item_definition("wooden_bow")
            .and_then(|d| d.ranged)
            .expect("wooden_bow has a ranged profile");
        let _ = ranged.update(0.0, true, true, Some(profile), true);
        let _ = ranged.update(0.5, false, true, Some(profile), true);
        let drawn = ranged_hud_view(&local, &ranged).expect("still holding the bow");
        match drawn.fill {
            Some(RangedHudFill::Draw(f)) => assert!(f > 0.0, "the bar fills with the draw"),
            other => panic!("expected a draw fill, got {other:?}"),
        }
    }

    #[test]
    fn ranged_hud_view_shows_reload_progress_for_the_crossbow() {
        let local = local_player_holding("crossbow", 3);
        let mut ranged = RangedDrawState::default();
        let profile = crate::items::item_definition("crossbow")
            .and_then(|d| d.ranged)
            .expect("crossbow has a ranged profile");
        // Fire and arm the reload the way the input layer does.
        let _ = ranged.update(0.0, true, true, Some(profile), true);
        ranged.begin_reload(profile);
        let _ = ranged.update(0.5, false, false, Some(profile), true);
        let view = ranged_hud_view(&local, &ranged).expect("crossbow shows the HUD");
        match view.fill {
            Some(RangedHudFill::Reload(f)) => {
                assert!(f > 0.0 && f < 1.0, "the bar tracks the reload, got {f}");
            }
            other => panic!("expected a reload fill, got {other:?}"),
        }
    }

    #[test]
    fn ranged_hud_paints_the_count_and_bar_when_the_actionbar_rect_is_known() {
        // A view with a live draw fill + a known actionbar rect paints shapes (the
        // ammo count text + the progress bar); this pins the painter path against
        // silently drawing nothing.
        let actionbar =
            egui::Rect::from_min_size(egui::pos2(300.0, 540.0), egui::vec2(200.0, 44.0));
        let ctx = egui::Context::default();
        let output = ctx.run(raw_input(), |ctx| {
            ranged_hud(
                ctx,
                &RangedHudView {
                    ammo: 12,
                    fill: Some(RangedHudFill::Draw(0.6)),
                },
                Some(actionbar),
            );
        });
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn ranged_hud_is_silent_without_a_known_actionbar_rect() {
        // Before the actionbar has been laid out (its rect is None), the ranged
        // HUD paints nothing rather than falling back to the crosshair.
        let ctx = egui::Context::default();
        let output = ctx.run(raw_input(), |ctx| {
            ranged_hud(
                ctx,
                &RangedHudView {
                    ammo: 12,
                    fill: Some(RangedHudFill::Draw(0.6)),
                },
                None,
            );
        });
        assert!(output.shapes.is_empty());
    }

    #[test]
    fn ranged_bar_sits_above_the_actionbar_and_fills_left_to_right() {
        let actionbar =
            egui::Rect::from_min_size(egui::pos2(300.0, 540.0), egui::vec2(200.0, 44.0));
        // Empty fill: the track spans the full actionbar width; the fill is
        // zero-width. Both sit entirely above the actionbar.
        let (track, fill) = ranged_bar_rects(actionbar, 0.0);
        assert!(
            track.bottom() <= actionbar.top(),
            "the bar sits above the actionbar"
        );
        assert!((track.left() - actionbar.left()).abs() < 1e-4);
        assert!((track.right() - actionbar.right()).abs() < 1e-4);
        assert!(fill.width() < 1e-4, "an empty draw has no fill");

        // Half fill: the fill covers the left half of the track and shares its top.
        let (track, fill) = ranged_bar_rects(actionbar, 0.5);
        assert!((fill.width() - track.width() * 0.5).abs() < 1e-3);
        assert!(
            (fill.left() - track.left()).abs() < 1e-4,
            "fills from the left"
        );

        // Full fill covers the whole track; over-unity fractions clamp.
        let (track, fill) = ranged_bar_rects(actionbar, 2.0);
        assert!((fill.width() - track.width()).abs() < 1e-3);
    }

    #[test]
    fn perf_overlay_is_gated_by_the_settings_toggle() {
        let runtime = ClientRuntime::default();
        let diagnostics = DiagnosticsStore::default();
        let voice = VoiceState::default();

        // Disabled: HUD with no local player and no perf toggle draws
        // nothing.
        let mut settings = ClientSettings::default();
        settings.hud.show_perf_stats = false;
        let ctx_off = egui::Context::default();
        let off = ctx_off.run(raw_input(), |ctx| {
            hud_ui(
                ctx,
                &runtime,
                &diagnostics,
                &settings,
                &voice,
                &CombatFeedbackState::default(),
                &LocalPlayerState::default(),
                &RangedDrawState::default(),
                &ThrowChargeState::default(),
                None,
            );
        });
        assert!(off.shapes.is_empty());

        // Enabled: the perf box renders even before the first server
        // PerfStats arrives ("waiting for server…").
        settings.hud.show_perf_stats = true;
        let ctx_on = egui::Context::default();
        let on = ctx_on.run(raw_input(), |ctx| {
            hud_ui(
                ctx,
                &runtime,
                &diagnostics,
                &settings,
                &voice,
                &CombatFeedbackState::default(),
                &LocalPlayerState::default(),
                &RangedDrawState::default(),
                &ThrowChargeState::default(),
                None,
            );
        });
        assert!(!on.shapes.is_empty());
    }

    #[test]
    fn perf_overlay_renders_with_server_stats() {
        use crate::protocol::{PerfClassificationId, PerfStatsSnapshot};

        let runtime = ClientRuntime {
            perf_stats: Some(PerfStatsSnapshot {
                loaded_chunks: 9,
                live_nodes: 42,
                pending_regrows: 3,
                aoi_visible_nodes: 7,
                player_chunk_x: -2,
                player_chunk_z: 5,
                player_classification: PerfClassificationId::Forest,
            }),
            ..Default::default()
        };
        let diagnostics = DiagnosticsStore::default();

        let ctx = egui::Context::default();
        let output = ctx.run(raw_input(), |ctx| {
            perf_stats_ui(ctx, &runtime, &diagnostics);
        });
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn frame_time_stats_defaults_to_zero_without_diagnostics() {
        let diagnostics = DiagnosticsStore::default();
        let stats = frame_time_stats(&diagnostics);
        assert_eq!(stats.fps, 0.0);
        assert_eq!(stats.last_ms, 0.0);
        assert_eq!(stats.p99_ms, 0.0);
        assert_eq!(stats.max_ms, 0.0);
    }

    #[test]
    fn connection_lag_indicator_draws_a_chip() {
        let ctx = egui::Context::default();
        let output = ctx.run(raw_input(), connection_lag_indicator);
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn voice_indicator_envelope_just_above_floor_hides() {
        // Envelope at/below the 0.005 floor is treated as idle.
        let ctx = egui::Context::default();
        let mut voice = VoiceState::default();
        voice.indicator_envelope = 0.004;
        let output = ctx.run(raw_input(), |ctx| voice_indicator(ctx, &voice));
        assert!(output.shapes.is_empty());
    }
}
