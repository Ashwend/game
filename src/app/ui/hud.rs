//! In-game HUD dispatcher. [`hud_ui`] owns the per-frame draw order of every
//! HUD widget; each widget lives in a focused submodule (ranged/throw/consume
//! readouts, meteor shower pill + warning, perf overlay, low-health vignette,
//! combat feedback, health bar). Only the small voice "transmitting" chip
//! stays here, next to the dispatcher.

mod combat_feedback;
mod health;
mod meteor_shower;
mod perf;
mod ranged;
mod vignette;

use bevy::diagnostic::DiagnosticsStore;
use bevy_egui::egui;

use crate::app::{
    state::{
        ClientRuntime, ClientSettings, CombatFeedbackState, ConsumeChargeState, LocalPlayerState,
        RangedDrawState, ThrowChargeState,
    },
    voice::VoiceState,
};

use self::combat_feedback::combat_feedback_ui;
use self::health::health_bar;
use self::meteor_shower::meteor_shower_hud;
use self::perf::{connection_lag_indicator, perf_stats_ui};
use self::ranged::{consume_hud_view, ranged_hud, ranged_hud_view, throw_hud_view};
use self::vignette::low_health_vignette;

use super::theme;

#[expect(clippy::too_many_arguments, reason = "egui UI plumbing")]
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
    consume: &ConsumeChargeState,
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
    } else if let Some(view) = consume_hud_view(local_player, consume) {
        // The bandage's use charge reuses the same bar again, with the held
        // bandage count as the "ammo".
        ranged_hud(ctx, &view, actionbar_rect);
    }

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

/// Minimal 800x600 egui input for the HUD widget tests, shared by this
/// module's tests and every submodule's.
#[cfg(test)]
fn raw_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{PlayerState, Vec3Net};

    fn player(health: f32) -> PlayerState {
        PlayerState {
            client_id: crate::protocol::ClientId(1),
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

        let _ = ctx.run_ui(raw_input(), |ui| {
            hud_ui(
                ui.ctx(),
                &runtime,
                &diagnostics,
                &ClientSettings::default(),
                &voice,
                &CombatFeedbackState::default(),
                &LocalPlayerState::default(),
                &RangedDrawState::default(),
                &ThrowChargeState::default(),
                &ConsumeChargeState::default(),
                None,
            );
        });

        // After Welcome seeds `predicted_local` the HUD has a view to
        // render. Snapshot used to drive this too, but Phase 6.2
        // removed the fallback.
        runtime.client_id = Some(crate::protocol::ClientId(1));
        runtime.predicted_local = Some(crate::controller::PlayerController::from_player_state(
            &player(75.0),
        ));

        let _ = ctx.run_ui(raw_input(), |ui| {
            hud_ui(
                ui.ctx(),
                &runtime,
                &diagnostics,
                &ClientSettings::default(),
                &voice,
                &CombatFeedbackState::default(),
                &LocalPlayerState::default(),
                &RangedDrawState::default(),
                &ThrowChargeState::default(),
                &ConsumeChargeState::default(),
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

        let output = ctx.run_ui(raw_input(), |ui| voice_indicator(ui.ctx(), &voice));
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn voice_indicator_hidden_when_idle() {
        let ctx = egui::Context::default();
        let voice = VoiceState::default();

        let output = ctx.run_ui(raw_input(), |ui| voice_indicator(ui.ctx(), &voice));
        // No voice activity → nothing drawn.
        assert!(output.shapes.is_empty());
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
        let off = ctx_off.run_ui(raw_input(), |ui| {
            hud_ui(
                ui.ctx(),
                &runtime,
                &diagnostics,
                &settings,
                &voice,
                &CombatFeedbackState::default(),
                &LocalPlayerState::default(),
                &RangedDrawState::default(),
                &ThrowChargeState::default(),
                &ConsumeChargeState::default(),
                None,
            );
        });
        assert!(off.shapes.is_empty());

        // Enabled: the perf box renders even before the first server
        // PerfStats arrives ("waiting for server…").
        settings.hud.show_perf_stats = true;
        let ctx_on = egui::Context::default();
        let on = ctx_on.run_ui(raw_input(), |ui| {
            hud_ui(
                ui.ctx(),
                &runtime,
                &diagnostics,
                &settings,
                &voice,
                &CombatFeedbackState::default(),
                &LocalPlayerState::default(),
                &RangedDrawState::default(),
                &ThrowChargeState::default(),
                &ConsumeChargeState::default(),
                None,
            );
        });
        assert!(!on.shapes.is_empty());
    }

    #[test]
    fn voice_indicator_envelope_just_above_floor_hides() {
        // Envelope at/below the 0.005 floor is treated as idle.
        let ctx = egui::Context::default();
        let mut voice = VoiceState::default();
        voice.indicator_envelope = 0.004;
        let output = ctx.run_ui(raw_input(), |ui| voice_indicator(ui.ctx(), &voice));
        assert!(output.shapes.is_empty());
    }
}
