//! Audio tab: master mix sliders plus test buttons that play a sample
//! through the live mix. Voice output volume lives on the dedicated
//! [Voice tab](super::voice_tab) so all voice-related controls sit in
//! one place.

use bevy_egui::egui;

use crate::app::{audio::SoundId, state::ClientSettings, ui::theme};

use super::widgets::{caption, percent_slider_row, section_label, setting_row};

/// egui temp-memory key for sample sequences queued by the test buttons.
/// `ui_system` drains this each frame (same drain point as the button
/// click/hover queue) and hands each `(delay, sound)` pair to the audio
/// scheduler, so the samples play at whatever the sliders say when each
/// one actually fires.
fn test_sound_queue_id() -> egui::Id {
    egui::Id::new("options_audio_test_sound_queue")
}

/// Seconds before the first sample fires. Covers the test button's own
/// UI click cue so the sample doesn't overlap it.
const TEST_START_DELAY_SECS: f32 = 0.35;
/// Spacing between effect samples: enough for each impact's tail to ring
/// out before the next one lands.
const TEST_EFFECT_SPACING_SECS: f32 = 0.55;
/// Spacing between footstep samples: an unhurried walking cadence, so
/// the sequence reads as actual footsteps rather than a drum roll.
const TEST_FOOTSTEP_SPACING_SECS: f32 = 0.42;

/// A short medley of gameplay impacts, the sounds the Effects slider
/// actually governs in play.
const EFFECTS_SAMPLE_SEQUENCE: [SoundId; 4] = [
    SoundId::ImpactAxeOnWood,
    SoundId::ImpactPickaxeOnStone,
    SoundId::ImpactAxeGeneric,
    SoundId::SwingMiss,
];

/// A few strides on dirt; the per-fire variant pool + pitch jitter keep
/// the steps from sounding like one repeated sample.
const FOOTSTEPS_SAMPLE_SEQUENCE: [SoundId; 5] = [
    SoundId::FootstepDirt,
    SoundId::FootstepDirt,
    SoundId::FootstepDirt,
    SoundId::FootstepDirt,
    SoundId::FootstepDirt,
];

fn queue_test_sequence(ui: &egui::Ui, sounds: &[SoundId], spacing_secs: f32) {
    ui.ctx().data_mut(|data| {
        let key = test_sound_queue_id();
        let mut queue = data
            .get_temp::<Vec<(f32, SoundId)>>(key)
            .unwrap_or_default();
        for (index, id) in sounds.iter().enumerate() {
            queue.push((TEST_START_DELAY_SECS + index as f32 * spacing_secs, *id));
        }
        data.insert_temp(key, queue);
    });
}

/// Drain the `(delay seconds, sound)` pairs queued during this frame's
/// draw pass. Called by `ui_system` after the panel renders; the pairs go
/// to the audio scheduler.
pub(in crate::app::ui) fn take_test_sounds(ctx: &egui::Context) -> Vec<(f32, SoundId)> {
    ctx.data_mut(|data| {
        data.remove_temp::<Vec<(f32, SoundId)>>(test_sound_queue_id())
            .unwrap_or_default()
    })
}

pub(super) fn render(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Mix"));
        ui.add_space(6.0);
        percent_slider_row(ui, "Master Volume", &mut settings.audio.master_volume);
        percent_slider_row(ui, "Music Volume", &mut settings.audio.music_volume);
        percent_slider_row(ui, "Effects Volume", &mut settings.audio.sfx_volume);
        percent_slider_row(ui, "Footsteps Volume", &mut settings.audio.footsteps_volume);
        percent_slider_row(ui, "Interface Volume", &mut settings.audio.ui_volume);

        ui.add_space(10.0);
        ui.label(section_label("Test"));
        ui.label(caption(
            "Plays a sample through the current Master mix so you can \
             check levels without leaving the menu.",
        ));
        ui.add_space(6.0);
        setting_row(ui, "Sample", |ui| {
            // Right-to-left layout: the rightmost button is added first.
            if theme::game_button(ui, "Footsteps", theme::ButtonKind::Secondary, 110.0).clicked() {
                queue_test_sequence(ui, &FOOTSTEPS_SAMPLE_SEQUENCE, TEST_FOOTSTEP_SPACING_SECS);
            }
            ui.add_space(8.0);
            if theme::game_button(ui, "Effects", theme::ButtonKind::Secondary, 110.0).clicked() {
                queue_test_sequence(ui, &EFFECTS_SAMPLE_SEQUENCE, TEST_EFFECT_SPACING_SECS);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::ClientSettings;

    fn run_tab(settings: &mut ClientSettings) -> (egui::Context, egui::FullOutput) {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    render(ui, settings);
                });
            },
        );
        (ctx, output)
    }

    #[test]
    fn mix_section_renders_all_volume_rows() {
        let mut settings = ClientSettings::default();
        let (_, output) = run_tab(&mut settings);
        assert!(!output.shapes.is_empty());
        assert_eq!(settings.audio.master_volume, 1.0);
        assert_eq!(settings.audio.footsteps_volume, 1.0);
    }

    #[test]
    fn test_sequences_space_samples_after_the_click_clears() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                queue_test_sequence(ui, &FOOTSTEPS_SAMPLE_SEQUENCE, TEST_FOOTSTEP_SPACING_SECS);
            });
        });
        let queued = take_test_sounds(&ctx);
        assert_eq!(queued.len(), FOOTSTEPS_SAMPLE_SEQUENCE.len());
        // Every entry waits out the button's own click cue first.
        assert!(
            queued
                .iter()
                .all(|(delay, _)| *delay >= TEST_START_DELAY_SECS)
        );
        // Strides are spaced at the walking cadence, in order.
        for pair in queued.windows(2) {
            let gap = pair[1].0 - pair[0].0;
            assert!((gap - TEST_FOOTSTEP_SPACING_SECS).abs() < 1e-5);
        }
        // Drained: a second take comes back empty.
        assert!(take_test_sounds(&ctx).is_empty());
    }
}
