//! Voice tab: enable/disable transmit, microphone gain, output gain.
//!
//! The audible-distance range is intentionally *not* a setting — it's a
//! gameplay rule fixed at [`crate::server::VOICE_AUDIBLE_RANGE`] so a
//! player can't extend or shrink it to gain a tactical advantage. The
//! "Audible Range" line below renders that value as static info so the
//! player can see what to expect without being able to edit it.

use bevy_egui::egui;

use crate::{
    app::{
        state::{ClientSettings, KeyAction, KeyBindings},
        ui::theme,
    },
    server::VOICE_AUDIBLE_RANGE,
};

use super::widgets::{
    caption, checkbox_with_click_sound, percent_slider_row, section_label, setting_row,
};

pub(super) fn render(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Voice Chat"));
        ui.add_space(4.0);
        ui.label(caption(&push_to_talk_hint(&settings.keybindings)));
        ui.add_space(6.0);
        setting_row(ui, "Enable Voice Chat", |ui| {
            checkbox_with_click_sound(ui, &mut settings.voice.enabled, "Enabled");
        });
        ui.add_enabled_ui(settings.voice.enabled, |ui| {
            percent_slider_row(ui, "Output Volume", &mut settings.voice.output_volume);
            percent_slider_row(ui, "Microphone Gain", &mut settings.voice.input_volume);
        });
        ui.add_space(4.0);
        setting_row(ui, "Audible Range", |ui| {
            ui.label(
                egui::RichText::new(format!("{:.0} m (game rule)", VOICE_AUDIBLE_RANGE))
                    .color(theme::muted_text()),
            );
        });
    });
}

fn push_to_talk_hint(bindings: &KeyBindings) -> String {
    let label = KeyBindings::slot_label(bindings.primary(KeyAction::PushToTalk));
    format!(
        "Hold {label} to transmit. Re-bind it on the Keybindings tab. Voices fade with distance and are only transmitted to players close enough to hear them."
    )
}
