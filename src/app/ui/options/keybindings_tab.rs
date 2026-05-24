//! Keybindings tab: list of every rebindable action with its current
//! primary/secondary key, a per-row reset button, and a global "Reset All
//! Defaults" button at the bottom. Clicking a slot pops the row into
//! capture mode — the next physical key press is recorded as the new
//! binding, any other action holding the same key is automatically
//! cleared, and the player can press Escape to bail.
//!
//! Layout mirrors the rest of the options tabs: each row is a horizontal
//! strip with the action label hugging the left edge and the
//! primary/secondary/reset controls packed against the right edge via a
//! `right_to_left` sub-layout. The label fills whatever space is left in
//! between, so resizing the panel just stretches the gap rather than
//! shifting any column.

use bevy::input::ButtonInput;
use bevy::prelude::KeyCode;
use bevy_egui::egui;

use crate::app::{
    state::{
        ClientSettings, KeyAction, KeyBindingCategory, KeyBindingSlot, KeyBindings, OptionsUiState,
        PendingRebind,
    },
    ui::theme,
};

use super::widgets::{SETTING_ROW_HEIGHT, section_label};

const KEY_SLOT_WIDTH: f32 = 140.0;
const RESET_BUTTON_WIDTH: f32 = 72.0;
const COLUMN_SPACING: f32 = 8.0;
const ROW_SPACING: f32 = 6.0;

pub(super) fn render(
    ui: &mut egui::Ui,
    settings: &mut ClientSettings,
    options_ui_state: &mut OptionsUiState,
    physical_keys: &ButtonInput<KeyCode>,
) {
    consume_pending_rebind(settings, options_ui_state, physical_keys);

    let categories = [
        KeyBindingCategory::Movement,
        KeyBindingCategory::Combat,
        KeyBindingCategory::Inventory,
        KeyBindingCategory::Communication,
    ];

    for (idx, category) in categories.iter().enumerate() {
        if idx > 0 {
            ui.add_space(10.0);
        }
        theme::inset_frame().show(ui, |ui| {
            // Force the section to fill the panel so every category frame is
            // the same width. Without this the frame shrinks to fit its
            // contents and visibly floats inside the panel.
            ui.set_width(ui.available_width());
            ui.label(section_label(category.label()));
            ui.add_space(6.0);
            render_category_rows(ui, settings, options_ui_state, *category);
        });
    }

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::compact_button(ui, "Reset All Defaults", theme::ButtonKind::Danger, 170.0)
                .clicked()
            {
                settings.keybindings.reset_all();
                options_ui_state.pending_rebind = None;
            }
        });
    });
}

fn render_category_rows(
    ui: &mut egui::Ui,
    settings: &mut ClientSettings,
    options_ui_state: &mut OptionsUiState,
    category: KeyBindingCategory,
) {
    column_header_row(ui);
    for action in KeyAction::ALL {
        if action.category() != category {
            continue;
        }
        ui.add_space(ROW_SPACING);
        action_row(ui, &mut settings.keybindings, options_ui_state, *action);
    }
}

/// Header strip above the action rows. Mirrors the action row layout so the
/// "Primary" / "Secondary" captions sit directly above their button columns.
/// The action column itself is intentionally unlabeled — the label text is
/// self-evident from each row.
fn column_header_row(ui: &mut egui::Ui) {
    keybinding_row(ui, "", |ui| {
        // Right-to-left, so add in reverse visual order. The empty allocation
        // over the reset column keeps "Secondary" aligned with the secondary
        // button column below.
        ui.allocate_space(egui::vec2(RESET_BUTTON_WIDTH, SETTING_ROW_HEIGHT));
        ui.add_space(COLUMN_SPACING);
        column_header_cell(ui, "Secondary");
        ui.add_space(COLUMN_SPACING);
        column_header_cell(ui, "Primary");
    });
}

fn column_header_cell(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(KEY_SLOT_WIDTH, SETTING_ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(12.0)
                    .color(theme::muted_text())
                    .strong(),
            );
        },
    );
}

/// One label-left / controls-right row. `add_controls` runs inside a
/// right-to-left sub-layout, so widgets must be added in **reverse visual
/// order** (rightmost first).
fn keybinding_row(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    add_controls: impl FnOnce(&mut egui::Ui),
) {
    let row_width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(row_width, SETTING_ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add(egui::Label::new(label).wrap_mode(egui::TextWrapMode::Extend));
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                add_controls,
            );
        },
    );
}

fn action_row(
    ui: &mut egui::Ui,
    bindings: &mut KeyBindings,
    options_ui_state: &mut OptionsUiState,
    action: KeyAction,
) {
    let slots = bindings.slots(action);
    let primary = slots.primary;
    let secondary = slots.secondary;
    let default_slots = action.default_slots();
    let is_default = primary == default_slots.primary && secondary == default_slots.secondary;

    keybinding_row(ui, theme::muted(action.label()), |ui| {
        // Right-to-left: Reset is rightmost, then Secondary, then Primary.
        let reset_clicked = ui
            .add_enabled_ui(!is_default, |ui| {
                slot_styled_button(
                    ui,
                    "Reset",
                    theme::ButtonKind::Secondary,
                    RESET_BUTTON_WIDTH,
                )
            })
            .inner
            .clicked();
        if reset_clicked {
            bindings.reset(action);
            options_ui_state.pending_rebind = None;
        }
        ui.add_space(COLUMN_SPACING);
        slot_button(
            ui,
            bindings,
            options_ui_state,
            action,
            KeyBindingSlot::Secondary,
            secondary,
        );
        ui.add_space(COLUMN_SPACING);
        slot_button(
            ui,
            bindings,
            options_ui_state,
            action,
            KeyBindingSlot::Primary,
            primary,
        );
    });
}

fn slot_button(
    ui: &mut egui::Ui,
    bindings: &mut KeyBindings,
    options_ui_state: &mut OptionsUiState,
    action: KeyAction,
    slot: KeyBindingSlot,
    current: Option<KeyCode>,
) {
    let is_capturing = options_ui_state.pending_rebind == Some(PendingRebind { action, slot });
    let label = if is_capturing {
        capture_label(ui.ctx())
    } else {
        KeyBindings::slot_label(current)
    };
    let kind = if is_capturing {
        theme::ButtonKind::Primary
    } else {
        theme::ButtonKind::Secondary
    };
    let response = slot_styled_button(ui, &label, kind, KEY_SLOT_WIDTH)
        .on_hover_text("Click to rebind. Right-click to clear.");
    if response.clicked() {
        options_ui_state.pending_rebind = Some(PendingRebind { action, slot });
    }
    if response.secondary_clicked() {
        bindings.set(action, slot, None);
        options_ui_state.pending_rebind = None;
    }
}

/// Render a button that fills exactly `width × SETTING_ROW_HEIGHT` and uses
/// the project palette for the given [`theme::ButtonKind`]. `min_size` is
/// what forces the rect to the requested width even when the label is
/// short — it's the difference between "all rows aligned" and "rows
/// staggering with label length".
fn slot_styled_button(
    ui: &mut egui::Ui,
    label: &str,
    kind: theme::ButtonKind,
    width: f32,
) -> egui::Response {
    let (fill, stroke, text_color) = theme::button_paint_rest(kind);
    let button = egui::Button::new(egui::RichText::new(label).size(13.0).color(text_color))
        .min_size(egui::vec2(width, SETTING_ROW_HEIGHT))
        .fill(fill)
        .stroke(stroke)
        .corner_radius(4)
        .sense(egui::Sense::click_and_drag());
    let response = ui.add(button);
    theme::record_click_sound(ui, &response);
    response
}

/// Animated three-dot pulse rendered while the slot is waiting for a key.
fn capture_label(ctx: &egui::Context) -> String {
    let phase = (ctx.input(|input| input.time) * 1.2) % 1.0;
    let dots = match (phase * 3.0) as usize {
        0 => "Press a key.",
        1 => "Press a key..",
        _ => "Press a key...",
    };
    ctx.request_repaint();
    dots.to_owned()
}

/// Reads the next physical key press while a rebind is pending. The capture
/// is intentionally tied to the Bevy keyboard input rather than egui's
/// input — egui's key events are filtered by IME and modifier state and
/// would miss raw bindings (`ShiftLeft` alone, function keys, etc.).
fn consume_pending_rebind(
    settings: &mut ClientSettings,
    options_ui_state: &mut OptionsUiState,
    physical_keys: &ButtonInput<KeyCode>,
) {
    let Some(pending) = options_ui_state.pending_rebind else {
        return;
    };
    // Escape cancels the capture. Handled at the panel root, but we
    // double-check here so a stale state doesn't outlive the modal.
    if physical_keys.just_pressed(KeyCode::Escape) {
        options_ui_state.pending_rebind = None;
        return;
    }
    let Some(code) = physical_keys.get_just_pressed().next().copied() else {
        return;
    };
    settings
        .keybindings
        .set(pending.action, pending.slot, Some(code));
    settings.keybindings.clear_conflicts(code, pending.action);
    options_ui_state.pending_rebind = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::{KeyAction, KeyBindingSlot};

    #[test]
    fn pressing_a_key_completes_pending_rebind() {
        let mut settings = ClientSettings::default();
        let mut state = OptionsUiState {
            tab: crate::app::state::OptionsTab::Keybindings,
            pending_rebind: Some(PendingRebind {
                action: KeyAction::Jump,
                slot: KeyBindingSlot::Primary,
            }),
        };
        let mut keys = ButtonInput::default();
        keys.press(KeyCode::KeyZ);

        consume_pending_rebind(&mut settings, &mut state, &keys);

        assert_eq!(
            settings.keybindings.primary(KeyAction::Jump),
            Some(KeyCode::KeyZ)
        );
        assert!(state.pending_rebind.is_none());
    }

    #[test]
    fn escape_during_capture_clears_pending() {
        let mut settings = ClientSettings::default();
        let mut state = OptionsUiState {
            tab: crate::app::state::OptionsTab::Keybindings,
            pending_rebind: Some(PendingRebind {
                action: KeyAction::Jump,
                slot: KeyBindingSlot::Primary,
            }),
        };
        let mut keys = ButtonInput::default();
        keys.press(KeyCode::Escape);
        consume_pending_rebind(&mut settings, &mut state, &keys);
        assert!(state.pending_rebind.is_none());
        assert_eq!(
            settings.keybindings.primary(KeyAction::Jump),
            Some(KeyCode::Space)
        );
    }

    #[test]
    fn rebinding_a_used_key_clears_the_other_action() {
        let mut settings = ClientSettings::default();
        let mut state = OptionsUiState {
            tab: crate::app::state::OptionsTab::Keybindings,
            pending_rebind: Some(PendingRebind {
                action: KeyAction::Jump,
                slot: KeyBindingSlot::Primary,
            }),
        };
        let mut keys = ButtonInput::default();
        keys.press(KeyCode::KeyV);
        consume_pending_rebind(&mut settings, &mut state, &keys);

        assert_eq!(
            settings.keybindings.primary(KeyAction::Jump),
            Some(KeyCode::KeyV)
        );
        assert_eq!(settings.keybindings.primary(KeyAction::PushToTalk), None);
    }
}
