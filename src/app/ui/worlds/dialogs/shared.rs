use bevy_egui::egui;

use crate::save::validate_world_name;

use super::super::super::modal;
use super::super::super::theme::{self, ButtonKind, COMPACT_ROW_HEIGHT};

/// The two outcomes a confirm/cancel modal form can signal. Shared by the
/// create and edit world dialogs (and any future one) so they map Enter and
/// click-outside the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfirmCancel {
    Confirm,
    Cancel,
}

/// One frame's outcome from [`confirm_modal`].
pub(super) enum ModalDecision {
    /// User confirmed (primary button or Enter).
    Confirm,
    /// User cancelled (Cancel button or click outside).
    Cancel,
    /// No decision yet. `finished_closing` is true on the frame the fade-out
    /// animation completes, the caller's cue to take + apply the dialog.
    Pending { finished_closing: bool },
}

/// Run a confirm/cancel modal: draw `body` inside the standard modal shell,
/// then fold the Enter shortcut into `Confirm` and click-outside into `Cancel`.
/// `open` is usually `!dialog.closing`. The `body` closure sets its
/// `&mut Option<ConfirmCancel>` when its own buttons are clicked.
pub(super) fn confirm_modal(
    ctx: &egui::Context,
    id: &'static str,
    open: bool,
    width: f32,
    height: f32,
    body: impl FnOnce(&mut egui::Ui, &mut Option<ConfirmCancel>),
) -> ModalDecision {
    let output = modal::modal_shell(ctx, id, open, width, height, body);

    let mut choice = output.choice;
    if choice.is_none() && output.confirm_shortcut_pressed {
        choice = Some(ConfirmCancel::Confirm);
    }
    if choice.is_none() && output.clicked_outside {
        choice = Some(ConfirmCancel::Cancel);
    }

    match choice {
        Some(ConfirmCancel::Confirm) => ModalDecision::Confirm,
        Some(ConfirmCancel::Cancel) => ModalDecision::Cancel,
        None => ModalDecision::Pending {
            finished_closing: output.finished_closing,
        },
    }
}

/// Draw the shared "Name" text field row used by the create and edit dialogs:
/// label + input, one-shot autofocus, select-all on focus, and per-keystroke
/// inline validation into `error`. Returns whether the current name is valid
/// (the caller uses it to enable/disable the primary button).
pub(super) fn name_field(
    ui: &mut egui::Ui,
    name: &mut String,
    error: &mut Option<String>,
    autofocus_pending: &mut bool,
    input_id: &str,
) -> bool {
    let mut name_changed = false;
    ui.horizontal(|ui| {
        field_label(ui, "Name");
        let name_response = ui.add_sized(
            [ui.available_width(), COMPACT_ROW_HEIGHT],
            theme::text_input(name).id(egui::Id::new(input_id)),
        );
        // Grab focus on the dialog's first frame so the player can type and
        // press Enter without clicking the field first. One-shot.
        if *autofocus_pending {
            name_response.request_focus();
            *autofocus_pending = false;
        }
        if name_response.gained_focus() {
            select_all_text(ui, name_response.id, name.chars().count());
        }
        if name_response.changed() {
            name_changed = true;
        }
    });

    // Refresh the inline error every keystroke so the user sees their typo
    // disappear the moment they fix it, rather than only on the next submit.
    let validation = validate_world_name(name);
    let is_valid = validation.is_ok();
    if name_changed {
        *error = validation.err().map(str::to_owned);
    }
    is_valid
}

/// Render an inline error line below a dialog form, if present.
pub(super) fn error_line(ui: &mut egui::Ui, error: Option<&String>) {
    if let Some(error) = error {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(error)
                .size(13.0)
                .color(theme::error_text()),
        );
    }
}

/// Render the shared right-aligned Cancel / primary button row. Sets `*choice`
/// when a button is clicked; the primary button is greyed out when
/// `primary_enabled` is false.
pub(super) fn confirm_button_row(
    ui: &mut egui::Ui,
    primary_label: &str,
    primary_enabled: bool,
    choice: &mut Option<ConfirmCancel>,
) {
    ui.add_space(18.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_enabled_ui(primary_enabled, |ui| {
            if theme::compact_button(ui, primary_label, ButtonKind::Primary, 92.0).clicked() {
                *choice = Some(ConfirmCancel::Confirm);
            }
        });
        if theme::compact_button(ui, "Cancel", ButtonKind::Secondary, 92.0).clicked() {
            *choice = Some(ConfirmCancel::Cancel);
        }
    });
}

pub(super) fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.add_sized(
        [88.0, COMPACT_ROW_HEIGHT],
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
