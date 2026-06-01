use bevy_egui::egui;

use uuid::Uuid;

use crate::{
    analytics::{Analytics, Event},
    app::state::{CreateWorldDialog, CurrentUser, MenuState, SaveStore},
    net::ClientNetwork,
    save::validate_world_name,
    world::ProceduralMapSize,
};

use super::super::super::{
    modal,
    theme::{self, ButtonKind, COMPACT_ROW_HEIGHT},
};
use super::super::session::{refresh_worlds, start_singleplayer_in_background};
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
    user: &CurrentUser,
    network: &ClientNetwork,
    analytics: &Analytics,
) {
    let finished_closing;
    {
        let Some(dialog) = menu.create_world.as_mut() else {
            return;
        };

        let output = create_world_modal(ctx, dialog, !dialog.closing);
        if let Some(choice) = output.choice {
            match choice {
                CreateWorldChoice::Create => {
                    match (validate_world_name(&dialog.name), dialog.selected_map()) {
                        (Ok(_), Ok(_)) => {
                            dialog.error = None;
                            dialog.closing = true;
                            dialog.confirmed = true;
                            ctx.request_repaint();
                        }
                        (Err(error), _) => {
                            dialog.error = Some(error.to_owned());
                            ctx.request_repaint();
                        }
                        (_, Err(error)) => {
                            dialog.error = Some(error.to_owned());
                            ctx.request_repaint();
                        }
                    }
                }
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
    if dialog.confirmed
        && let Some(world_id) = create_world_from_dialog(dialog, menu, store, user, analytics)
    {
        // Creating a world almost always means "play it now", so drop straight
        // into the fresh save instead of bouncing back to the list. The world
        // is already in `menu.worlds` (create refreshes the list), so the
        // loading splash can resolve its name. Reuse the exact background
        // start the table's Start button drives.
        analytics.track(Event::WorldLoaded);
        start_singleplayer_in_background(menu, store, user, network, world_id);
    }
}

/// Create + persist the world described by `dialog`, refresh the list, and
/// return the new world's id on success. Kept free of the start/host concern
/// so it stays a pure persistence step (the caller decides whether to enter
/// the world); `create_world_dialog_ui` drives the immediate join.
pub(in crate::app::ui::worlds) fn create_world_from_dialog(
    dialog: CreateWorldDialog,
    menu: &mut MenuState,
    store: &SaveStore,
    user: &CurrentUser,
    analytics: &Analytics,
) -> Option<Uuid> {
    let map = match dialog.selected_map() {
        Ok(map) => map,
        Err(error) => {
            menu.status = Some(error.to_owned());
            return None;
        }
    };

    let map_type = format!("{:?}", map);
    match store
        .0
        .create_world_with_map(&dialog.name, Some(user.0.account_id), map)
    {
        Ok(save) => {
            analytics.track(Event::WorldCreated { map_type });
            refresh_worlds(menu, store);
            Some(save.id)
        }
        Err(error) => {
            menu.status = Some(format!("create failed: {error}"));
            None
        }
    }
}

fn create_world_modal(
    ctx: &egui::Context,
    dialog: &mut CreateWorldDialog,
    open: bool,
) -> CreateWorldModalOutput {
    let output = modal::modal_shell(
        ctx,
        "create_world_modal",
        open,
        340.0,
        480.0,
        |ui, choice| {
            draw_create_world_form(ui, dialog, choice);
        },
    );

    let mut choice = output.choice;
    if choice.is_none() && output.confirm_shortcut_pressed {
        choice = Some(CreateWorldChoice::Create);
    }
    if choice.is_none() && output.clicked_outside {
        choice = Some(CreateWorldChoice::Cancel);
    }

    CreateWorldModalOutput {
        choice,
        finished_closing: output.finished_closing,
    }
}

fn draw_create_world_form(
    ui: &mut egui::Ui,
    dialog: &mut CreateWorldDialog,
    choice: &mut Option<CreateWorldChoice>,
) {
    ui.label(theme::section("Create World"));
    ui.add_space(12.0);

    let mut name_changed = false;
    ui.horizontal(|ui| {
        field_label(ui, "Name");
        let name_response = ui.add_sized(
            [ui.available_width(), COMPACT_ROW_HEIGHT],
            theme::text_input(&mut dialog.name).id(egui::Id::new(CREATE_WORLD_NAME_INPUT_ID)),
        );
        if name_response.gained_focus() {
            select_all_text(ui, name_response.id, dialog.name.chars().count());
        }
        if name_response.changed() {
            name_changed = true;
        }
    });

    // Refresh the inline error every keystroke so the user sees their typo
    // disappear the moment they fix it, rather than only on the next submit.
    // `name_is_valid` is detached from `dialog.name`'s borrow so the rest of
    // the form (which needs `&mut dialog`) can keep mutating freely.
    let name_is_valid = {
        let validation = validate_world_name(&dialog.name);
        if name_changed {
            dialog.error = validation.err().map(str::to_owned);
        }
        validation.is_ok()
    };

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
            [seed_width, COMPACT_ROW_HEIGHT],
            theme::text_input(&mut dialog.seed).id(egui::Id::new(CREATE_WORLD_SEED_INPUT_ID)),
        );
        if theme::compact_button(ui, "Refresh", ButtonKind::Secondary, 82.0).clicked() {
            dialog.refresh_seed();
        }
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
            if theme::compact_button(ui, "Create", ButtonKind::Primary, 92.0).clicked() {
                *choice = Some(CreateWorldChoice::Create);
            }
        });
        if theme::compact_button(ui, "Cancel", ButtonKind::Secondary, 92.0).clicked() {
            *choice = Some(CreateWorldChoice::Cancel);
        }
    });
}
