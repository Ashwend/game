//! General tab: HUD toggles and other small "doesn't fit a bigger category"
//! flags. Lives behind a dedicated tab so adding a future toggle (e.g.
//! crosshair style) doesn't pollute Display or Audio.

use bevy_egui::egui;

use crate::app::{state::ClientSettings, ui::theme};

use super::widgets::{checkbox_with_click_sound, section_label, setting_row};

pub(super) fn render(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Interface"));
        ui.add_space(6.0);
        setting_row(ui, "FPS Counter", |ui| {
            checkbox_with_click_sound(ui, &mut settings.hud.show_fps, "Enabled");
        });
    });
}
