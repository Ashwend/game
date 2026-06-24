//! Shared row layout + form widgets used by every tab in the options panel.
//! Pulled out so each tab module stays focused on what it controls.

use bevy_egui::egui;

use crate::app::ui::theme;

pub(super) const SETTING_LABEL_WIDTH: f32 = 200.0;
pub(super) const SETTING_CONTROL_WIDTH: f32 = 300.0;
pub(super) const SETTING_ROW_HEIGHT: f32 = 36.0;

/// Section header inside a tab body. Slightly larger and brighter than the
/// row labels so the eye can find the start of each grouping.
pub(super) fn section_label(label: &str) -> egui::RichText {
    egui::RichText::new(label)
        .size(14.0)
        .strong()
        .color(egui::Color32::from_rgb(196, 216, 236))
}

/// Slim caption below a section label, used to set context for the next
/// few rows (e.g. "Voice is transmitted only when Push To Talk is held.").
pub(super) fn caption(text: &str) -> egui::RichText {
    egui::RichText::new(text)
        .size(12.0)
        .color(theme::muted_text())
}

/// One left-label / right-control row.
pub(super) fn setting_row(ui: &mut egui::Ui, label: &str, add_control: impl FnOnce(&mut egui::Ui)) {
    let row_width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(row_width, SETTING_ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(SETTING_LABEL_WIDTH, SETTING_ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(theme::muted(label));
                },
            );

            let spacer = (ui.available_width() - SETTING_CONTROL_WIDTH).max(0.0);
            if spacer > 0.0 {
                ui.add_space(spacer);
            }
            ui.allocate_ui_with_layout(
                egui::vec2(
                    SETTING_CONTROL_WIDTH.min(ui.available_width()),
                    SETTING_ROW_HEIGHT,
                ),
                egui::Layout::right_to_left(egui::Align::Center),
                add_control,
            );
        },
    );
}

pub(super) fn percent_slider_row(ui: &mut egui::Ui, label: &str, value: &mut f32) {
    let mut percent = *value * 100.0;
    setting_row(ui, label, |ui| {
        let control_width = ui.available_width();
        if ui
            .add_sized(
                [control_width, SETTING_ROW_HEIGHT],
                egui::Slider::new(&mut percent, 0.0..=100.0)
                    .suffix("%")
                    .show_value(true),
            )
            .changed()
        {
            *value = (percent / 100.0).clamp(0.0, 1.0);
        }
    });
}

/// A labelled `f32` slider row showing the live numeric value, for free-range dev
/// tuning (unlike [`percent_slider_row`], the value is the raw number, not 0-100%).
/// `step` of `0.0` lets the slider move continuously.
pub(super) fn value_slider_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f64,
    decimals: usize,
) {
    setting_row(ui, label, |ui| {
        let control_width = ui.available_width();
        ui.add_sized(
            [control_width, SETTING_ROW_HEIGHT],
            egui::Slider::new(value, range)
                .step_by(step)
                .max_decimals(decimals)
                .show_value(true),
        );
    });
}

pub(super) fn checkbox_with_click_sound(ui: &mut egui::Ui, value: &mut bool, label: &str) {
    let response = ui
        .checkbox(value, label)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    theme::record_click_sound(ui, &response);
}
