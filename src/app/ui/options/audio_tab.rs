//! Audio tab: master mix sliders. Voice output volume lives on the
//! dedicated [Voice tab](super::voice_tab) so all voice-related controls
//! sit in one place.

use bevy_egui::egui;

use crate::app::{state::ClientSettings, ui::theme};

use super::widgets::{percent_slider_row, section_label};

pub(super) fn render(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Mix"));
        ui.add_space(6.0);
        percent_slider_row(ui, "Master Volume", &mut settings.audio.master_volume);
        percent_slider_row(ui, "Music Volume", &mut settings.audio.music_volume);
        percent_slider_row(ui, "Effects Volume", &mut settings.audio.sfx_volume);
        percent_slider_row(ui, "Interface Volume", &mut settings.audio.ui_volume);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::ClientSettings;

    #[test]
    fn mix_section_renders_all_volume_rows() {
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
        assert_eq!(settings.audio.master_volume, 1.0);
    }
}
