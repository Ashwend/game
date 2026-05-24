//! General tab: HUD toggles and other small "doesn't fit a bigger category"
//! flags. Lives behind a dedicated tab so adding a future toggle (e.g.
//! crosshair style) doesn't pollute Display or Audio.

use bevy_egui::egui;

use crate::{
    app::{state::ClientSettings, ui::theme},
    protocol::ViewRadiusTier,
};

use super::widgets::{checkbox_with_click_sound, section_label, setting_row};

pub(super) fn render(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Interface"));
        ui.add_space(6.0);
        setting_row(ui, "Performance Stats", |ui| {
            checkbox_with_click_sound(ui, &mut settings.hud.show_perf_stats, "Enabled (F2)");
        });
        setting_row(ui, "Chunk Overlay", |ui| {
            checkbox_with_click_sound(ui, &mut settings.hud.show_chunk_overlay, "Enabled");
        });
        setting_row(ui, "View Distance", |ui| {
            let response = egui::ComboBox::from_id_salt("options_view_radius")
                .selected_text(settings.hud.view_radius.label())
                .width(230.0)
                .show_ui(ui, |ui| {
                    for tier in ViewRadiusTier::ALL {
                        let response =
                            ui.selectable_value(&mut settings.hud.view_radius, tier, tier.label());
                        theme::record_click_sound(ui, &response);
                    }
                })
                .response;
            theme::record_click_sound(ui, &response);
        });
    });
}
