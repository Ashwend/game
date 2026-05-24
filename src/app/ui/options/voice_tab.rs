//! Voice tab: enable/disable transmit, microphone gain, output gain.

use bevy_egui::egui;

use crate::app::{
    state::{ClientSettings, KeyAction, KeyBindings},
    ui::theme,
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
    });
}

fn push_to_talk_hint(bindings: &KeyBindings) -> String {
    let Some(code) = bindings.primary(KeyAction::PushToTalk) else {
        return "Push To Talk is unbound. Assign a key on the Keybindings tab to transmit. Voices fade with distance and are only transmitted to players close enough to hear them.".to_owned();
    };
    let raw = KeyBindings::slot_label(Some(code));
    let label = humanize_key_label(&raw);
    format!(
        "Hold the {label} key to transmit. Re-bind it on the Keybindings tab. Voices fade with distance and are only transmitted to players close enough to hear them."
    )
}

/// Strip Bevy's `KeyCode` naming prefixes so a setting reads like a key on a
/// keyboard ("V", "3") instead of like an enum variant ("KeyV", "Digit3").
fn humanize_key_label(raw: &str) -> String {
    raw.strip_prefix("Key")
        .or_else(|| raw.strip_prefix("Digit"))
        .unwrap_or(raw)
        .to_owned()
}
