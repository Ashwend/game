use bevy_egui::egui;

use crate::{
    app::state::{CreateWorldDialog, CreateWorldMapKind, MenuState, SaveStore, SteamUser},
    world::ProceduralMapSize,
};

use super::super::super::theme::{self, ButtonKind};
use super::super::{BUTTON_HEIGHT, session::refresh_worlds};
use super::shared::{field_label, select_all_text};

const CREATE_WORLD_NAME_INPUT_ID: &str = "create_world_name_input";
const CREATE_WORLD_SEED_INPUT_ID: &str = "create_world_seed_input";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateWorldChoice {
    Create,
    Cancel,
}

#[derive(Debug, Clone, Copy)]
struct CreateWorldModalOutput {
    choice: Option<CreateWorldChoice>,
    finished_closing: bool,
}

pub(in crate::app::ui::worlds) fn open_create_world_dialog(menu: &mut MenuState) {
    menu.create_world = Some(CreateWorldDialog::new());
}

pub(in crate::app::ui::worlds) fn create_world_dialog_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    store: &SaveStore,
    user: &SteamUser,
) {
    let finished_closing;
    {
        let Some(dialog) = menu.create_world.as_mut() else {
            return;
        };

        let output = create_world_modal(ctx, dialog, !dialog.closing);
        if let Some(choice) = output.choice {
            match choice {
                CreateWorldChoice::Create => match dialog.selected_map() {
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
                CreateWorldChoice::Cancel => {
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

    let Some(dialog) = menu.create_world.take() else {
        return;
    };
    if dialog.confirmed {
        create_world_from_dialog(dialog, menu, store, user);
    }
}

pub(in crate::app::ui::worlds) fn create_world_from_dialog(
    dialog: CreateWorldDialog,
    menu: &mut MenuState,
    store: &SaveStore,
    user: &SteamUser,
) {
    let map = match dialog.selected_map() {
        Ok(map) => map,
        Err(error) => {
            menu.status = Some(error.to_owned());
            return;
        }
    };

    match store
        .0
        .create_world_with_map(&dialog.name, Some(user.0.steam_id), map)
    {
        Ok(_) => refresh_worlds(menu, store),
        Err(error) => menu.status = Some(format!("create failed: {error}")),
    }
}

fn create_world_modal(
    ctx: &egui::Context,
    dialog: &mut CreateWorldDialog,
    open: bool,
) -> CreateWorldModalOutput {
    let id = egui::Id::new("create_world_modal");
    let animation = ctx.animate_bool_with_time(id.with("animation"), open, 0.16);
    if animation > 0.0 && animation < 1.0 {
        ctx.request_repaint();
    }

    if !open && animation <= 0.01 {
        return CreateWorldModalOutput {
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
                    draw_create_world_form(ui, dialog, &mut choice);
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
            choice = Some(CreateWorldChoice::Cancel);
        }
    }

    CreateWorldModalOutput {
        choice,
        finished_closing: false,
    }
}

fn draw_create_world_form(
    ui: &mut egui::Ui,
    dialog: &mut CreateWorldDialog,
    choice: &mut Option<CreateWorldChoice>,
) {
    ui.label(theme::section("Create World"));
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        field_label(ui, "Name");
        let name_response = ui.add_sized(
            [ui.available_width(), BUTTON_HEIGHT],
            theme::text_input(&mut dialog.name).id(egui::Id::new(CREATE_WORLD_NAME_INPUT_ID)),
        );
        if name_response.gained_focus() {
            select_all_text(ui, name_response.id, dialog.name.chars().count());
        }
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        field_label(ui, "Map Type");
        let test_response =
            ui.selectable_value(&mut dialog.map_kind, CreateWorldMapKind::Test, "Test");
        theme::record_click_sound(ui, &test_response);
        let procedural_response = ui.selectable_value(
            &mut dialog.map_kind,
            CreateWorldMapKind::Procedural,
            "Procedural",
        );
        theme::record_click_sound(ui, &procedural_response);
    });

    if dialog.map_kind == CreateWorldMapKind::Procedural {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            field_label(ui, "Map Size");
            for size in ProceduralMapSize::ALL {
                let response = ui.selectable_value(
                    &mut dialog.procedural_size,
                    size,
                    format!("{} ({:.0})", size.label(), size.floor_size()),
                );
                theme::record_click_sound(ui, &response);
            }
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            field_label(ui, "Seed");
            let seed_width = (ui.available_width() - 92.0).max(120.0);
            ui.add_sized(
                [seed_width, BUTTON_HEIGHT],
                theme::text_input(&mut dialog.seed).id(egui::Id::new(CREATE_WORLD_SEED_INPUT_ID)),
            );
            if theme::compact_button(ui, "Refresh", ButtonKind::Secondary, 82.0).clicked() {
                dialog.refresh_seed();
            }
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
        if theme::compact_button(ui, "Create", ButtonKind::Primary, 92.0).clicked() {
            *choice = Some(CreateWorldChoice::Create);
        }
        if theme::compact_button(ui, "Cancel", ButtonKind::Secondary, 92.0).clicked() {
            *choice = Some(CreateWorldChoice::Cancel);
        }
    });
}
