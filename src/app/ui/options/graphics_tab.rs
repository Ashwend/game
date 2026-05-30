//! Graphics tab: client-side rendering options (bloom, anti-aliasing, and —
//! added in later phases — sunshafts and grass density). HDR is a required
//! baseline for the atmosphere sky and so is intentionally not a toggle here.

use bevy_egui::egui;

use crate::app::{
    state::{AntiAliasing, ClientSettings, GrassDensity, ShadowQuality},
    ui::theme,
};

use super::widgets::{checkbox_with_click_sound, section_label, setting_row};

pub(super) fn render(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Post-processing"));
        ui.add_space(6.0);
        setting_row(ui, "Bloom", |ui| {
            checkbox_with_click_sound(ui, &mut settings.graphics.bloom_enabled, "Enabled");
        });
    });

    ui.add_space(10.0);

    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Quality"));
        ui.add_space(6.0);
        anti_aliasing_row(ui, settings);
        shadows_row(ui, settings);
        grass_row(ui, settings);
    });
}

fn grass_row(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    setting_row(ui, "Grass", |ui| {
        let response = egui::ComboBox::from_id_salt("options_grass_density")
            .selected_text(settings.graphics.grass_density.label())
            .width(230.0)
            .show_ui(ui, |ui| {
                for density in GrassDensity::ALL {
                    let response = ui.selectable_value(
                        &mut settings.graphics.grass_density,
                        density,
                        density.label(),
                    );
                    theme::record_click_sound(ui, &response);
                }
            })
            .response;
        theme::record_click_sound(ui, &response);
    });
}

fn shadows_row(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    setting_row(ui, "Shadows", |ui| {
        let response = egui::ComboBox::from_id_salt("options_shadows")
            .selected_text(settings.graphics.shadows.label())
            .width(230.0)
            .show_ui(ui, |ui| {
                for quality in ShadowQuality::ALL {
                    let response = ui.selectable_value(
                        &mut settings.graphics.shadows,
                        quality,
                        quality.label(),
                    );
                    theme::record_click_sound(ui, &response);
                }
            })
            .response;
        theme::record_click_sound(ui, &response);
    });
}

fn anti_aliasing_row(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    setting_row(ui, "Anti-aliasing", |ui| {
        let response = egui::ComboBox::from_id_salt("options_anti_aliasing")
            .selected_text(settings.graphics.anti_aliasing.label())
            .width(230.0)
            .show_ui(ui, |ui| {
                for mode in AntiAliasing::ALL {
                    let response = ui.selectable_value(
                        &mut settings.graphics.anti_aliasing,
                        mode,
                        mode.label(),
                    );
                    theme::record_click_sound(ui, &response);
                }
            })
            .response;
        theme::record_click_sound(ui, &response);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::ClientSettings;

    #[test]
    fn graphics_tab_renders_with_defaults_intact() {
        let ctx = egui::Context::default();
        let mut settings = ClientSettings::default();

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
                    render(ui, &mut settings);
                });
            },
        );

        assert!(!output.shapes.is_empty());
        // Defaults survive a render with no interaction.
        assert!(settings.graphics.bloom_enabled);
        assert_eq!(settings.graphics.anti_aliasing, AntiAliasing::Fxaa);
    }
}
