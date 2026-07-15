//! Perf stats overlay (the F2 box: frame-time triplet, mesh counts, server
//! chunk stats) plus the connection-lag chip. The F2 toggle itself lives in
//! `src/app/systems/input/menu_toggles.rs`; this module only renders.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy_egui::egui;

use crate::app::{state::ClientRuntime, systems::render_stats};

use super::super::theme;

/// Fixed width of the perf overlay so chunk labels like
/// `(-99, -99) Rocky outcrop` don't push the values column around.
const PERF_BOX_WIDTH: f32 = 240.0;
const PERF_LABEL_WIDTH: f32 = 96.0;
const PERF_VALUE_WIDTH: f32 = 124.0;

/// Small chip rendered in the top-left when the session has gone silent
/// long enough to be suspicious. Stays out of the way during normal play
/// but is immediately visible the moment the link goes wobbly.
pub(super) fn connection_lag_indicator(ctx: &egui::Context) {
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

pub(super) fn perf_stats_ui(
    ctx: &egui::Context,
    runtime: &ClientRuntime,
    diagnostics: &DiagnosticsStore,
) {
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
                    // Client-side render load: total `Mesh3d` entities vs how many
                    // survive culling (drawn in the main view or a shadow cascade).
                    // Unlike the server's `Visible` node count (AoI ring, view-
                    // independent), this shows if the scene is drawn wholesale.
                    if let (Some(total), Some(drawn)) = (
                        diagnostics
                            .get(&render_stats::MESH_TOTAL)
                            .and_then(|d| d.value()),
                        diagnostics
                            .get(&render_stats::MESH_VISIBLE)
                            .and_then(|d| d.value()),
                    ) {
                        label(
                            ui,
                            "Meshes",
                            format!("{} drawn / {}", drawn as u32, total as u32),
                        );
                    }
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

#[cfg(test)]
mod tests {
    use super::super::raw_input;
    use super::*;

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
        let output = ctx.run_ui(raw_input(), |ui| {
            perf_stats_ui(ui.ctx(), &runtime, &diagnostics);
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
        let output = ctx.run_ui(raw_input(), |ui| connection_lag_indicator(ui.ctx()));
        assert!(!output.shapes.is_empty());
    }
}
