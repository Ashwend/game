//! Controls tab: mouse-related settings that aren't tied to a key. The
//! actual key map is on the Keybindings tab so each rebindable action has
//! a single source of truth.

use bevy_egui::egui;

use crate::app::{state::ClientSettings, ui::theme};

use super::widgets::{SETTING_ROW_HEIGHT, checkbox_with_click_sound, section_label, setting_row};

pub(super) fn render(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Mouse"));
        ui.add_space(6.0);
        sensitivity_row(ui, settings);
        setting_row(ui, "Invert Mouse Y", |ui| {
            checkbox_with_click_sound(ui, &mut settings.input.invert_mouse_y, "Enabled");
        });
    });
}

fn sensitivity_row(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    let mut value = settings.input.mouse_sensitivity * 100.0;
    setting_row(ui, "Mouse Sensitivity", |ui| {
        let control_width = ui.available_width();
        if ui
            .add_sized(
                [control_width, SETTING_ROW_HEIGHT],
                egui::Slider::new(&mut value, 25.0..=300.0)
                    .suffix("%")
                    .show_value(true),
            )
            .changed()
        {
            settings.input.mouse_sensitivity = (value / 100.0).clamp(0.25, 3.0);
        }
    });
}
