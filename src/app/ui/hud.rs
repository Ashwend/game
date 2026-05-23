use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy_egui::egui;

use crate::{
    app::{
        state::{ClientRuntime, ClientSettings},
        voice::VoiceState,
    },
    protocol::MAX_HEALTH,
};

use super::theme;

const HEALTH_WIDTH: f32 = 192.0;
const HEALTH_HEIGHT: f32 = 30.0;
const HEALTH_ICON_WIDTH: f32 = 30.0;
const FPS_COUNTER_WIDTH: f32 = 58.0;
const FPS_COUNTER_HEIGHT: f32 = 16.0;

pub(super) fn hud_ui(
    ctx: &egui::Context,
    runtime: &ClientRuntime,
    diagnostics: &DiagnosticsStore,
    settings: &ClientSettings,
    voice: &VoiceState,
) {
    if settings.hud.show_fps {
        fps_ui(ctx, diagnostics);
    }
    if runtime.connection_is_lagging() {
        connection_lag_indicator(ctx);
    }

    voice_indicator(ctx, voice);

    let Some(player) = runtime.local_view() else {
        return;
    };

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
                        // U+25CF — a hand-drawn circle has no font dep.
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

fn fps_ui(ctx: &egui::Context, diagnostics: &DiagnosticsStore) {
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|diagnostic| diagnostic.smoothed())
        .unwrap_or_default();

    egui::Area::new("fps_counter".into())
        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 14.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(6, 9, 13, 150))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(115, 132, 151, 60),
                ))
                .corner_radius(5)
                .inner_margin(egui::Margin::symmetric(9, 5))
                .show(ui, |ui| {
                    ui.set_min_size(egui::vec2(FPS_COUNTER_WIDTH, FPS_COUNTER_HEIGHT));
                    ui.add_sized(
                        [FPS_COUNTER_WIDTH, FPS_COUNTER_HEIGHT],
                        egui::Label::new(
                            egui::RichText::new(format!("{fps:.0} FPS"))
                                .monospace()
                                .size(12.0)
                                .color(theme::muted_text()),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
        });
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
    use crate::protocol::{PlayerState, Vec3Net, WorldSnapshot};

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
            steam_id: 1,
            name: "Player".to_owned(),
            position: Vec3Net::new(1.0, 2.0, 3.0),
            velocity: Vec3Net::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            health,
            grounded: true,
            last_processed_input: 0,
            is_admin: false,
            chat_bubble: None,
            inventory: None,
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
            );
        });

        runtime.client_id = Some(1);
        runtime.snapshot = Some(WorldSnapshot {
            tick: 1,
            players: vec![player(75.0)],
            dropped_items: Vec::new(),
            resource_nodes: Vec::new(),
        });

        let _ = ctx.run(raw_input(), |ctx| {
            hud_ui(
                ctx,
                &runtime,
                &diagnostics,
                &ClientSettings::default(),
                &voice,
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
}
