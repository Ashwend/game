use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy_egui::egui;

use crate::{
    app::{
        state::{ClientRuntime, ClientSettings, CombatFeedbackState},
        voice::VoiceState,
    },
    protocol::MAX_HEALTH,
};

use super::theme;

const HEALTH_WIDTH: f32 = 192.0;
const HEALTH_HEIGHT: f32 = 30.0;
const HEALTH_ICON_WIDTH: f32 = 30.0;
/// Fixed width of the perf overlay so chunk labels like
/// `(-99, -99) Rocky outcrop` don't push the values column around.
const PERF_BOX_WIDTH: f32 = 240.0;
const PERF_LABEL_WIDTH: f32 = 96.0;
const PERF_VALUE_WIDTH: f32 = 124.0;

pub(super) fn hud_ui(
    ctx: &egui::Context,
    runtime: &ClientRuntime,
    diagnostics: &DiagnosticsStore,
    settings: &ClientSettings,
    voice: &VoiceState,
    combat: &CombatFeedbackState,
) {
    if settings.hud.show_perf_stats {
        perf_stats_ui(ctx, runtime, diagnostics);
    }
    if runtime.connection_is_lagging() {
        connection_lag_indicator(ctx);
    }

    voice_indicator(ctx, voice);

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

    egui::Area::new("hud_bars".into())
        .anchor(egui::Align2::RIGHT_BOTTOM, [-18.0, -18.0])
        .show(ctx, |ui| {
            health_bar(ui, player.health);
        });
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
            low_health_vignette(ctx, MAX_HEALTH * 0.1)
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
