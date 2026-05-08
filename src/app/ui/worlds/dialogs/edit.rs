use bevy_egui::egui;

use crate::{
    app::state::{EditWorldDialog, MenuState, SaveStore},
    world::MapType,
};

use super::super::super::theme::{self, ButtonKind};
use super::super::{BUTTON_HEIGHT, session::refresh_worlds};
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
            dialog.closing = true;
            dialog.confirmed = choice == EditWorldChoice::Save;
            ctx.request_repaint();
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
    let id = egui::Id::new("edit_world_modal");
    let animation = ctx.animate_bool_with_time(id.with("animation"), open, 0.16);
    if animation > 0.0 && animation < 1.0 {
        ctx.request_repaint();
    }

    if !open && animation <= 0.01 {
        return EditWorldModalOutput {
            choice: None,
            finished_closing: true,
        };
    }

    let screen_rect = ctx.content_rect();
    let backdrop_response = egui::Area::new(id.with("backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let local_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, screen_rect.size());
            let response = ui.allocate_rect(local_rect, egui::Sense::click());
            ui.painter().rect_filled(
                local_rect,
                0,
                egui::Color32::from_rgba_unmultiplied(1, 3, 8, (190.0 * animation) as u8),
            );
            response
        })
        .inner;

    let panel_width = screen_rect.width().clamp(340.0, 480.0);
    let mut choice = None;
    let panel_response = egui::Area::new(id.with("panel"))
        .order(egui::Order::Tooltip)
        .anchor(
            egui::Align2::CENTER_CENTER,
            [0.0, 18.0 * (1.0 - animation.clamp(0.0, 1.0))],
        )
        .show(ctx, |ui| {
            ui.set_width(panel_width);
            ui.multiply_opacity(animation);
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(12, 17, 23, 246))
                .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
                .corner_radius(7)
                .inner_margin(egui::Margin::symmetric(24, 22))
                .show(ui, |ui| {
                    ui.set_width(panel_width - 48.0);
                    draw_edit_world_form(ui, dialog, &mut choice);
                });
        })
        .response;

    if open && choice.is_none() && backdrop_response.clicked() {
        let clicked_outside_panel = ctx.input(|input| {
            input
                .pointer
                .interact_pos()
                .is_some_and(|position| !panel_response.rect.contains(position))
        });
        if clicked_outside_panel {
            choice = Some(EditWorldChoice::Cancel);
        }
    }

    EditWorldModalOutput {
        choice,
        finished_closing: false,
    }
}

fn draw_edit_world_form(
    ui: &mut egui::Ui,
    dialog: &mut EditWorldDialog,
    choice: &mut Option<EditWorldChoice>,
) {
    ui.label(theme::section("Edit World"));
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        field_label(ui, "Name");
        let name_response = ui.add_sized(
            [ui.available_width(), BUTTON_HEIGHT],
            theme::text_input(&mut dialog.name).id(egui::Id::new(EDIT_WORLD_NAME_INPUT_ID)),
        );
        if name_response.gained_focus() {
            select_all_text(ui, name_response.id, dialog.name.chars().count());
        }
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, "Map Type");
        locked_setting(ui, dialog.map.label(), 116.0);
    });

    if let MapType::Procedural { seed, size } = &dialog.map {
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
    }

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
        if theme::compact_button(ui, "Save", ButtonKind::Primary, 92.0).clicked() {
            *choice = Some(EditWorldChoice::Save);
        }
        if theme::compact_button(ui, "Cancel", ButtonKind::Secondary, 92.0).clicked() {
            *choice = Some(EditWorldChoice::Cancel);
        }
    });
}

fn locked_setting(ui: &mut egui::Ui, text: &str, width: f32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, BUTTON_HEIGHT), egui::Sense::hover());
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
