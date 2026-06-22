//! Voice tab: enable/disable transmit, microphone + output gain, input/output
//! device selection, and a mic test (live level meter + optional self-loopback).

use bevy_egui::egui;

use crate::app::{
    state::{ClientSettings, KeyAction, KeyBindings},
    ui::theme,
};

use super::VoiceTabIo;
use super::widgets::{
    caption, checkbox_with_click_sound, percent_slider_row, section_label, setting_row,
};

pub(super) fn render(ui: &mut egui::Ui, settings: &mut ClientSettings, io: &mut VoiceTabIo) {
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

    ui.add_space(10.0);
    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Devices"));
        ui.add_space(4.0);
        if !io.playback_available {
            ui.label(
                egui::RichText::new(
                    "Audio output device unavailable: you will not hear other players. \
                     Pick a different output below.",
                )
                .size(12.0)
                .color(egui::Color32::from_rgb(236, 170, 120)),
            );
            ui.add_space(4.0);
        } else {
            ui.label(caption(
                "Leave a device on System Default to follow your OS choice. A saved \
                 device that gets unplugged falls back to the default automatically.",
            ));
            ui.add_space(4.0);
        }
        device_combo(
            ui,
            "Microphone",
            &mut settings.voice.input_device,
            &io.devices.inputs,
            "voice_input_device",
        );
        device_combo(
            ui,
            "Output",
            &mut settings.voice.output_device,
            &io.devices.outputs,
            "voice_output_device",
        );
        setting_row(ui, "Device List", |ui| {
            if theme::game_button(ui, "Refresh", theme::ButtonKind::Secondary, 110.0).clicked() {
                io.control.refresh_requested = true;
            }
        });
    });

    ui.add_space(10.0);
    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Test Microphone"));
        ui.add_space(4.0);
        ui.label(caption(
            "Turn the test on and speak: the bar tracks your microphone level. \
             Enable Hear Myself (use headphones) to play your own mic back to you.",
        ));
        ui.add_space(6.0);
        setting_row(ui, "Test Microphone", |ui| {
            checkbox_with_click_sound(ui, &mut io.control.test_active, "On");
        });
        // Live level meter, fed from whatever capture is currently open.
        let level = io.input_level.clamp(0.0, 1.0);
        ui.add(
            egui::ProgressBar::new(level)
                .fill(egui::Color32::from_rgb(120, 200, 130))
                .desired_width(ui.available_width()),
        );
        ui.add_space(4.0);
        ui.add_enabled_ui(io.control.test_active, |ui| {
            setting_row(ui, "Hear Myself", |ui| {
                checkbox_with_click_sound(ui, &mut io.control.loopback, "On");
            });
        });
    });
}

/// One device-picker row: a combo box whose first entry is "System Default"
/// (mapped to `None`) followed by every enumerated device name.
fn device_combo(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut Option<String>,
    devices: &[String],
    id_salt: &str,
) {
    setting_row(ui, label, |ui| {
        let current_label = selected
            .clone()
            .unwrap_or_else(|| "System Default".to_owned());
        let control_width = ui.available_width();
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(current_label)
            .width(control_width)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(selected.is_none(), "System Default")
                    .clicked()
                {
                    *selected = None;
                }
                for name in devices {
                    let is_selected = selected.as_deref() == Some(name.as_str());
                    if ui.selectable_label(is_selected, name).clicked() {
                        *selected = Some(name.clone());
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::voice::{VoiceDeviceCache, VoiceUiControl};

    #[test]
    fn voice_tab_renders_with_devices_and_dead_output_warning() {
        let ctx = egui::Context::default();
        let mut settings = ClientSettings::default();
        let mut devices = VoiceDeviceCache::default();
        devices.inputs = vec!["Built-in Microphone".to_owned(), "Headset".to_owned()];
        devices.outputs = vec!["Built-in Output".to_owned()];
        let mut control = VoiceUiControl::default();
        let mut io = VoiceTabIo {
            devices: &devices,
            control: &mut control,
            input_level: 0.4,
            // false exercises the "output unavailable" warning branch.
            playback_available: false,
        };
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    render(ui, &mut settings, &mut io);
                });
            },
        );
        assert!(!output.shapes.is_empty());
    }
}
