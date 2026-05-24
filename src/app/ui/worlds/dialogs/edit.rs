use bevy_egui::egui;

use crate::{
    app::state::{EditWorldDialog, MenuState, SaveStore},
    save::validate_world_name,
    world::MapType,
};

use super::super::super::{
    modal,
    theme::{self, ButtonKind, COMPACT_ROW_HEIGHT},
};
use super::super::session::refresh_worlds;
use super::shared::{field_label, select_all_text};

const EDIT_WORLD_NAME_INPUT_ID: &str = "edit_world_name_input";
const LOCKED_SETTING_TOOLTIP_TITLE: &str = "Locked Setting";
const LOCKED_SETTING_TOOLTIP_BODY: &str =
    "World generation settings cannot be changed after the world has been created.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditWorldChoice {
    Save,
    Cancel,
}

#[derive(Debug, Clone, Copy)]
struct EditWorldModalOutput {
    choice: Option<EditWorldChoice>,
    finished_closing: bool,
}

pub(in crate::app::ui::worlds) fn edit_world_dialog_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    store: &SaveStore,
) {
    let finished_closing;
    {
        let Some(dialog) = menu.edit_world.as_mut() else {
            return;
        };

        let output = edit_world_modal(ctx, dialog, !dialog.closing);
        if let Some(choice) = output.choice {
            match choice {
                EditWorldChoice::Save => match validate_world_name(&dialog.name) {
                    Ok(_) => {
                        dialog.error = None;
                        dialog.closing = true;
                        dialog.confirmed = true;
                        ctx.request_repaint();
                    }
                    Err(error) => {
                        dialog.error = Some(error.to_owned());
                        ctx.request_repaint();
                    }
                },
                EditWorldChoice::Cancel => {
                    dialog.closing = true;
                    dialog.confirmed = false;
                    ctx.request_repaint();
                }
            }
        }
        finished_closing = output.finished_closing;
    }

    if !finished_closing {
        return;
    }

    let Some(dialog) = menu.edit_world.take() else {
        return;
    };
    if dialog.confirmed {
        rename_world_from_dialog(dialog, menu, store);
    }
}

pub(in crate::app::ui::worlds) fn rename_world_from_dialog(
    dialog: EditWorldDialog,
    menu: &mut MenuState,
    store: &SaveStore,
) {
    match store.0.rename_world(dialog.world_id, &dialog.name) {
        Ok(_) => refresh_worlds(menu, store),
        Err(error) => menu.status = Some(format!("rename failed: {error}")),
    }
}

fn edit_world_modal(
    ctx: &egui::Context,
    dialog: &mut EditWorldDialog,
    open: bool,
) -> EditWorldModalOutput {
    let output = modal::modal_shell(ctx, "edit_world_modal", open, 340.0, 480.0, |ui, choice| {
        draw_edit_world_form(ui, dialog, choice);
    });

    let mut choice = output.choice;
    if choice.is_none() && output.confirm_shortcut_pressed {
        choice = Some(EditWorldChoice::Save);
    }
    if choice.is_none() && output.clicked_outside {
        choice = Some(EditWorldChoice::Cancel);
    }

    EditWorldModalOutput {
        choice,
        finished_closing: output.finished_closing,
    }
}

fn draw_edit_world_form(
    ui: &mut egui::Ui,
    dialog: &mut EditWorldDialog,
    choice: &mut Option<EditWorldChoice>,
) {
    ui.label(theme::section("Edit World"));
    ui.add_space(12.0);

    let mut name_changed = false;
    ui.horizontal(|ui| {
        field_label(ui, "Name");
        let name_response = ui.add_sized(
            [ui.available_width(), COMPACT_ROW_HEIGHT],
            theme::text_input(&mut dialog.name).id(egui::Id::new(EDIT_WORLD_NAME_INPUT_ID)),
        );
        if name_response.gained_focus() {
            select_all_text(ui, name_response.id, dialog.name.chars().count());
        }
        if name_response.changed() {
            name_changed = true;
        }
    });

    let name_is_valid = {
        let validation = validate_world_name(&dialog.name);
        if name_changed {
            dialog.error = validation.err().map(str::to_owned);
        }
        validation.is_ok()
    };

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, "Map Type");
        locked_setting(ui, dialog.map.label(), 116.0);
    });

    let MapType::Procedural { seed, size } = &dialog.map;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, "Map Size");
        locked_setting(
            ui,
            &format!("{} ({:.0})", size.label(), size.floor_size()),
            126.0,
        );
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, "Seed");
        locked_setting(ui, &seed.to_string(), ui.available_width());
    });

    if let Some(error) = &dialog.error {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(error)
                .size(13.0)
                .color(egui::Color32::from_rgb(255, 154, 130)),
        );
    }

    ui.add_space(18.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_enabled_ui(name_is_valid, |ui| {
            if theme::compact_button(ui, "Save", ButtonKind::Primary, 92.0).clicked() {
                *choice = Some(EditWorldChoice::Save);
            }
        });
        if theme::compact_button(ui, "Cancel", ButtonKind::Secondary, 92.0).clicked() {
            *choice = Some(EditWorldChoice::Cancel);
        }
    });
}

fn locked_setting(ui: &mut egui::Ui, text: &str, width: f32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, COMPACT_ROW_HEIGHT), egui::Sense::hover());
    ui.painter().rect(
        rect,
        4,
        egui::Color32::from_rgba_unmultiplied(28, 32, 38, 190),
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(92, 102, 116, 72)),
        egui::StrokeKind::Inside,
    );
    ui.painter().with_clip_rect(rect).text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        theme::muted_text(),
    );
    theme::wow_tooltip(
        response,
        LOCKED_SETTING_TOOLTIP_TITLE,
        LOCKED_SETTING_TOOLTIP_BODY,
    )
}
