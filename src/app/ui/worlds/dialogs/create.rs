use bevy_egui::egui;

use uuid::Uuid;

use crate::{
    analytics::{Analytics, Event},
    app::state::{CreateWorldDialog, CurrentUser, MenuState, SaveStore},
    net::ClientNetwork,
    save::validate_world_name,
    world::ProceduralMapSize,
};

use super::super::super::theme::{self, ButtonKind, COMPACT_ROW_HEIGHT};
use super::super::session::{refresh_worlds, start_singleplayer_in_background};
use super::shared::{
    ConfirmCancel, ModalDecision, confirm_button_row, confirm_modal, error_line, field_label,
    name_field,
};

const CREATE_WORLD_NAME_INPUT_ID: &str = "create_world_name_input";
const CREATE_WORLD_SEED_INPUT_ID: &str = "create_world_seed_input";

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

        match confirm_modal(
            ctx,
            "create_world_modal",
            !dialog.closing,
            340.0,
            480.0,
            |ui, choice| {
                draw_create_world_form(ui, dialog, choice);
            },
        ) {
            ModalDecision::Confirm => {
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
                finished_closing = false;
            }
            ModalDecision::Cancel => {
                dialog.closing = true;
                dialog.confirmed = false;
                ctx.request_repaint();
                finished_closing = false;
            }
            ModalDecision::Pending {
                finished_closing: fc,
            } => finished_closing = fc,
        }
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

    let map_type = format!("{map:?}");
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

fn draw_create_world_form(
    ui: &mut egui::Ui,
    dialog: &mut CreateWorldDialog,
    choice: &mut Option<ConfirmCancel>,
) {
    ui.label(theme::section("Create World"));
    ui.add_space(12.0);

    let name_is_valid = name_field(
        ui,
        &mut dialog.name,
        &mut dialog.error,
        &mut dialog.autofocus_pending,
        CREATE_WORLD_NAME_INPUT_ID,
    );

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

    error_line(ui, dialog.error.as_ref());
    confirm_button_row(ui, "Create", name_is_valid, choice);
}
