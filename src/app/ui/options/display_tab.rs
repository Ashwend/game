//! Display tab: window mode, resolution, vsync.

use bevy::window::Monitor;
use bevy_egui::egui;

use crate::app::{
    state::{
        ClientSettings, DisplayMode, MAX_FOV_DEG, MAX_UI_SCALE, MIN_FOV_DEG, MIN_UI_SCALE,
        display_resolutions,
    },
    ui::theme,
};

use super::widgets::{SETTING_ROW_HEIGHT, checkbox_with_click_sound, section_label, setting_row};

pub(super) fn render(
    ui: &mut egui::Ui,
    settings: &mut ClientSettings,
    primary_monitor: Option<&Monitor>,
) {
    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Window"));
        ui.add_space(6.0);
        display_mode_row(ui, settings);
        resolution_row(ui, settings, primary_monitor);
        setting_row(ui, "VSync", |ui| {
            checkbox_with_click_sound(ui, &mut settings.display.vsync, "Enabled");
        });
    });

    ui.add_space(10.0);

    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("View"));
        ui.add_space(6.0);
        fov_row(ui, settings);
        ui_scale_row(ui, settings);
    });
}

fn fov_row(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    let mut degrees = settings.display.fov_degrees.clamp(MIN_FOV_DEG, MAX_FOV_DEG);
    setting_row(ui, "Field of View", |ui| {
        let control_width = ui.available_width();
        if ui
            .add_sized(
                [control_width, SETTING_ROW_HEIGHT],
                egui::Slider::new(&mut degrees, MIN_FOV_DEG..=MAX_FOV_DEG)
                    .suffix("\u{00b0}")
                    .show_value(true),
            )
            .changed()
        {
            settings.display.fov_degrees = degrees.clamp(MIN_FOV_DEG, MAX_FOV_DEG);
        }
    });
}

fn ui_scale_row(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    let mut percent = settings.display.ui_scale.clamp(MIN_UI_SCALE, MAX_UI_SCALE) * 100.0;
    setting_row(ui, "UI Scale", |ui| {
        let control_width = ui.available_width();
        if ui
            .add_sized(
                [control_width, SETTING_ROW_HEIGHT],
                egui::Slider::new(
                    &mut percent,
                    (MIN_UI_SCALE * 100.0)..=(MAX_UI_SCALE * 100.0),
                )
                .suffix("%")
                .show_value(true),
            )
            .changed()
        {
            settings.display.ui_scale = (percent / 100.0).clamp(MIN_UI_SCALE, MAX_UI_SCALE);
        }
    });
}

fn display_mode_row(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    setting_row(ui, "Display Mode", |ui| {
        let response = egui::ComboBox::from_id_salt("options_display_mode")
            .selected_text(settings.display.mode.label())
            .width(230.0)
            .show_ui(ui, |ui| {
                for mode in DisplayMode::ALL {
                    let response =
                        ui.selectable_value(&mut settings.display.mode, mode, mode.label());
                    theme::record_click_sound(ui, &response);
                }
            })
            .response;
        theme::record_click_sound(ui, &response);
    });
}

fn resolution_row(
    ui: &mut egui::Ui,
    settings: &mut ClientSettings,
    primary_monitor: Option<&Monitor>,
) {
    let mut resolutions = display_resolutions(primary_monitor, settings.display.mode);
    if settings.display.mode != DisplayMode::Fullscreen
        && !resolutions.contains(&settings.display.resolution)
    {
        resolutions.push(settings.display.resolution);
    }
    resolutions.sort_by_key(|resolution| {
        (
            u64::from(resolution.width) * u64::from(resolution.height),
            resolution.width,
            resolution.height,
        )
    });

    if settings.display.mode == DisplayMode::Fullscreen
        && !resolutions.contains(&settings.display.resolution)
        && let Some(resolution) = resolutions.last().copied()
    {
        settings.display.resolution = resolution;
    }

    let enabled = settings.display.mode != DisplayMode::BorderlessFullscreen;
    let selected_text = if enabled {
        settings.display.resolution.label()
    } else {
        "Native Display".to_owned()
    };

    setting_row(ui, "Resolution", |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            let response = egui::ComboBox::from_id_salt("options_resolution")
                .selected_text(selected_text)
                .width(230.0)
                .show_ui(ui, |ui| {
                    for resolution in resolutions {
                        let response = ui.selectable_value(
                            &mut settings.display.resolution,
                            resolution,
                            resolution.label(),
                        );
                        theme::record_click_sound(ui, &response);
                    }
                })
                .response;
            theme::record_click_sound(ui, &response);
        });
    });
    let _ = SETTING_ROW_HEIGHT; // used by the slider widgets in other tabs.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borderless_fullscreen_disables_resolution_selection() {
        let ctx = egui::Context::default();
        let mut settings = ClientSettings::default();
        settings.display.mode = DisplayMode::BorderlessFullscreen;

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
                    resolution_row(ui, &mut settings, None);
                });
            },
        );

        assert!(!output.shapes.is_empty());
        assert_eq!(settings.display.mode, DisplayMode::BorderlessFullscreen);
    }

    #[test]
    fn view_section_renders_fov_and_ui_scale_rows() {
        let ctx = egui::Context::default();
        let mut settings = ClientSettings::default();

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
                    render(ui, &mut settings, None);
                });
            },
        );

        assert!(!output.shapes.is_empty());
        // Defaults survive a render with no interaction.
        assert_eq!(settings.display.fov_degrees, 65.0);
        assert_eq!(settings.display.ui_scale, 1.0);
    }
}
