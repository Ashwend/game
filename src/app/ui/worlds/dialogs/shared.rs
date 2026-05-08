use bevy_egui::egui;

use super::super::super::theme;
use super::super::BUTTON_HEIGHT;

pub(super) fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.add_sized(
        [88.0, BUTTON_HEIGHT],
        egui::Label::new(theme::field_label(text)),
    );
}

pub(super) fn select_all_text(ui: &egui::Ui, id: egui::Id, char_count: usize) {
    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::default(),
            egui::text::CCursor::new(char_count),
        )));
    state.store(ui.ctx(), id);
}
