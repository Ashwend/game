use anyhow::Context;
use bevy_egui::egui;
use uuid::Uuid;

use crate::{
    app::state::{
        ClientRuntime, ConfirmationDialog, CreateWorldDialog, CreateWorldMapKind, EditWorldDialog,
        MenuState, SaveStore, Screen, SteamUser,
    },
    net::ClientSession,
    world::{MapType, ProceduralMapSize},
};

use super::theme::{self, ButtonKind};

const INSET_FRAME_HORIZONTAL_PADDING: f32 = 28.0;
const ROW_HEIGHT: f32 = 60.0;
const ROW_HORIZONTAL_PADDING: f32 = 14.0;
const COLUMN_GAP: f32 = 18.0;
const START_BUTTON_WIDTH: f32 = 78.0;
const EDIT_BUTTON_WIDTH: f32 = 64.0;
const DELETE_BUTTON_WIDTH: f32 = 82.0;
const ACTION_BUTTON_GAP: f32 = 10.0;
const BUTTON_HEIGHT: f32 = 34.0;
const CREATE_WORLD_NAME_INPUT_ID: &str = "create_world_name_input";
const CREATE_WORLD_SEED_INPUT_ID: &str = "create_world_seed_input";
const EDIT_WORLD_NAME_INPUT_ID: &str = "edit_world_name_input";
const LOCKED_SETTING_TOOLTIP_TITLE: &str = "Locked Setting";
const LOCKED_SETTING_TOOLTIP_BODY: &str =
    "World generation settings cannot be changed after the world has been created.";

pub(super) fn worlds_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
) {
    theme::screen_scrim(ctx, "worlds_scrim", 145);
    handle_worlds_escape(ctx, menu);
    theme::anchored_panel(
        ctx,
        "worlds_panel",
        920.0,
        egui::Align2::CENTER_CENTER,
        [0.0, -8.0],
        |ui| {
            let has_worlds = !menu.worlds.is_empty();
            ui.horizontal(|ui| {
                ui.label(theme::section("Singleplayer Worlds"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::compact_button(ui, "Back", ButtonKind::Secondary, 78.0).clicked() {
                        menu.screen = Screen::MainMenu;
                    }
                    if has_worlds
                        && theme::compact_button(ui, "Create New World", ButtonKind::Primary, 142.0)
                            .clicked()
                    {
                        open_create_world_dialog(menu);
                    }
                });
            });

            ui.add_space(16.0);
            draw_world_headers(ui);
            let table_height = table_height(ctx);
            draw_world_table(ui, menu, runtime, store, user, table_height);

            if let Some(status) = &menu.status {
                ui.add_space(10.0);
                ui.label(theme::status_text(status));
            }
        },
    );
    create_world_dialog_ui(ctx, menu, store, user);
    edit_world_dialog_ui(ctx, menu, store);
}

fn handle_worlds_escape(ctx: &egui::Context, menu: &mut MenuState) {
    if !ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        return;
    }

    if let Some(dialog) = menu.create_world.as_mut() {
        dialog.closing = true;
        dialog.confirmed = false;
        ctx.request_repaint();
        return;
    }

    if let Some(dialog) = menu.edit_world.as_mut() {
        dialog.closing = true;
        dialog.confirmed = false;
        ctx.request_repaint();
        return;
    }

    if let Some(dialog) = menu.confirmation.as_mut() {
        dialog.closing = true;
        dialog.confirmed = false;
        ctx.request_repaint();
        return;
    }

    menu.screen = Screen::MainMenu;
}

fn table_height(ctx: &egui::Context) -> f32 {
    (ctx.content_rect().height() - 240.0).max(180.0)
}

fn draw_world_table(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
    table_height: f32,
) {
    let table_outer_width = ui.available_width();
    theme::inset_frame().show(ui, |ui| {
        let table_content_width = (table_outer_width - INSET_FRAME_HORIZONTAL_PADDING).max(0.0);
        ui.set_width(table_content_width);
        ui.set_min_height(table_height);
        if menu.worlds.is_empty() {
            draw_empty_worlds_state(ui, menu, table_content_width, table_height);
            return;
        }

        ui.allocate_ui_with_layout(
            egui::vec2(table_content_width, table_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(table_height)
                    .show(ui, |ui| {
                        ui.set_width(table_content_width);
                        let worlds = menu.worlds.clone();
                        for world in worlds {
                            draw_world_row(
                                ui,
                                menu,
                                runtime,
                                store,
                                user,
                                world,
                                table_content_width,
                            );
                            ui.add_space(8.0);
                        }
                    });
            },
        );
    });
}

fn draw_empty_worlds_state(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    table_content_width: f32,
    table_height: f32,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(table_content_width, table_height),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            let content_height = 14.0 + 8.0 + BUTTON_HEIGHT;
            ui.add_space(((table_height - content_height) * 0.5).max(24.0));
            ui.label(theme::muted("No worlds found."));
            ui.add_space(8.0);
            if theme::compact_button(ui, "Create New World", ButtonKind::Primary, 154.0).clicked() {
                open_create_world_dialog(menu);
            }
        },
    );
}

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

fn open_create_world_dialog(menu: &mut MenuState) {
    menu.create_world = Some(CreateWorldDialog::new());
}

fn create_world_dialog_ui(
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

fn create_world_from_dialog(
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

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.add_sized(
        [88.0, BUTTON_HEIGHT],
        egui::Label::new(theme::field_label(text)),
    );
}

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

fn edit_world_dialog_ui(ctx: &egui::Context, menu: &mut MenuState, store: &SaveStore) {
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

fn rename_world_from_dialog(dialog: EditWorldDialog, menu: &mut MenuState, store: &SaveStore) {
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

fn draw_world_headers(ui: &mut egui::Ui) {
    let header_width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(header_width, 22.0), egui::Sense::hover());
    let content_left = rect.left() + INSET_FRAME_HORIZONTAL_PADDING * 0.5 + ROW_HORIZONTAL_PADDING;
    let content_width =
        (header_width - INSET_FRAME_HORIZONTAL_PADDING - ROW_HORIZONTAL_PADDING * 2.0).max(0.0);
    let content_rect = egui::Rect::from_min_size(
        egui::pos2(content_left, rect.top()),
        egui::vec2(content_width, rect.height()),
    );
    let columns = WorldColumns::for_width(content_rect.width());
    draw_columns(
        ui,
        content_rect,
        columns,
        [
            HeaderCell::new("World"),
            HeaderCell::new("Map"),
            HeaderCell::new("Actions"),
        ],
    );
    ui.add_space(6.0);
}

fn draw_world_row(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
    world: crate::save::WorldSummary,
    row_outer_width: f32,
) {
    let row_width = row_outer_width.max(0.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(row_width, ROW_HEIGHT), egui::Sense::hover());
    let fill = egui::Color32::from_rgba_unmultiplied(7, 10, 14, 218);
    ui.painter().rect(
        rect,
        5,
        fill,
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(90, 108, 128, 92)),
        egui::StrokeKind::Inside,
    );

    let content_rect = rect.shrink2(egui::vec2(ROW_HORIZONTAL_PADDING, 0.0));
    let columns = WorldColumns::for_width(content_rect.width());
    let cells = column_rects(content_rect, columns);

    draw_cell_text(
        ui,
        cells.name,
        world.name.as_str(),
        theme::text(),
        14.0,
        egui::FontFamily::Proportional,
    );
    draw_cell_text(
        ui,
        cells.map,
        world.map.label(),
        theme::muted_text(),
        14.0,
        egui::FontFamily::Proportional,
    );

    let button_y = cells.actions.center().y;
    let start_rect = egui::Rect::from_min_size(
        egui::pos2(cells.actions.left(), button_y - BUTTON_HEIGHT * 0.5),
        egui::vec2(START_BUTTON_WIDTH, BUTTON_HEIGHT),
    );
    let edit_rect = egui::Rect::from_min_size(
        egui::pos2(
            start_rect.right() + ACTION_BUTTON_GAP,
            button_y - BUTTON_HEIGHT * 0.5,
        ),
        egui::vec2(EDIT_BUTTON_WIDTH, BUTTON_HEIGHT),
    );
    let delete_rect = egui::Rect::from_min_size(
        egui::pos2(
            edit_rect.right() + ACTION_BUTTON_GAP,
            button_y - BUTTON_HEIGHT * 0.5,
        ),
        egui::vec2(DELETE_BUTTON_WIDTH, BUTTON_HEIGHT),
    );

    if theme::compact_button_in_rect(
        ui,
        ("world-start", world.id),
        start_rect,
        "Start",
        ButtonKind::Primary,
    )
    .clicked()
    {
        start_singleplayer(menu, runtime, store, user, world.id);
    }
    if theme::compact_button_in_rect(
        ui,
        ("world-edit", world.id),
        edit_rect,
        "Edit",
        ButtonKind::Secondary,
    )
    .clicked()
    {
        menu.edit_world = Some(EditWorldDialog::new(&world));
    }
    if theme::compact_button_in_rect(
        ui,
        ("world-delete", world.id),
        delete_rect,
        "Delete",
        ButtonKind::Danger,
    )
    .clicked()
    {
        menu.confirmation = Some(ConfirmationDialog::delete_world(world.id, &world.name));
    }
}

#[derive(Debug, Clone, Copy)]
struct HeaderCell {
    text: &'static str,
}

impl HeaderCell {
    fn new(text: &'static str) -> Self {
        Self { text }
    }
}

#[derive(Debug, Clone, Copy)]
struct ColumnRects {
    name: egui::Rect,
    map: egui::Rect,
    actions: egui::Rect,
}

fn draw_columns(
    ui: &egui::Ui,
    content_rect: egui::Rect,
    columns: WorldColumns,
    headers: [HeaderCell; 3],
) {
    let cells = column_rects(content_rect, columns);
    draw_cell_text(
        ui,
        cells.name,
        headers[0].text,
        egui::Color32::from_rgb(172, 190, 208),
        12.0,
        egui::FontFamily::Proportional,
    );
    draw_cell_text(
        ui,
        cells.map,
        headers[1].text,
        egui::Color32::from_rgb(172, 190, 208),
        12.0,
        egui::FontFamily::Proportional,
    );
    draw_cell_text(
        ui,
        cells.actions,
        headers[2].text,
        egui::Color32::from_rgb(172, 190, 208),
        12.0,
        egui::FontFamily::Proportional,
    );
}

fn column_rects(content_rect: egui::Rect, columns: WorldColumns) -> ColumnRects {
    let mut x = content_rect.left();
    let name = cell_rect(content_rect, x, columns.name);
    x += columns.name + COLUMN_GAP;
    let map = cell_rect(content_rect, x, columns.map);
    x += columns.map + COLUMN_GAP;
    let actions = cell_rect(content_rect, x, columns.actions);

    ColumnRects { name, map, actions }
}

fn cell_rect(content_rect: egui::Rect, left: f32, width: f32) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(left, content_rect.top()),
        egui::vec2(width.max(0.0), content_rect.height()),
    )
}

fn draw_cell_text(
    ui: &egui::Ui,
    rect: egui::Rect,
    text: impl Into<String>,
    color: egui::Color32,
    size: f32,
    family: egui::FontFamily,
) {
    ui.painter().with_clip_rect(rect).text(
        egui::pos2(rect.left(), rect.center().y),
        egui::Align2::LEFT_CENTER,
        text.into(),
        egui::FontId::new(size, family),
        color,
    );
}

#[derive(Debug, Clone, Copy)]
struct WorldColumns {
    name: f32,
    map: f32,
    actions: f32,
}

impl WorldColumns {
    fn for_width(width: f32) -> Self {
        let actions = START_BUTTON_WIDTH
            + ACTION_BUTTON_GAP
            + EDIT_BUTTON_WIDTH
            + ACTION_BUTTON_GAP
            + DELETE_BUTTON_WIDTH;
        let remaining = (width - actions - COLUMN_GAP * 2.0).max(0.0);
        let map = 140.0_f32.min((remaining * 0.32).max(100.0));
        let name = (remaining - map).max(150.0);

        Self { name, map, actions }
    }
}

fn select_all_text(ui: &egui::Ui, id: egui::Id, char_count: usize) {
    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::default(),
            egui::text::CCursor::new(char_count),
        )));
    state.store(ui.ctx(), id);
}

pub(super) fn refresh_worlds(menu: &mut MenuState, store: &SaveStore) {
    match store.0.list_worlds() {
        Ok(worlds) => {
            menu.worlds = worlds;
            menu.status = None;
        }
        Err(error) => {
            menu.worlds.clear();
            menu.status = Some(format!("world list failed: {error}"));
        }
    }
}

fn start_singleplayer(
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
    world_id: Uuid,
) {
    let result = store
        .0
        .load_world(world_id)
        .context("could not load selected world")
        .and_then(|save| ClientSession::start_singleplayer(save, &user.0));

    match result {
        Ok(session) => {
            runtime.start_session(session, Some(world_id));
            menu.screen = Screen::InGame;
            menu.pause_open = false;
            menu.chat_open = false;
            menu.chat_focus_pending = false;
            menu.status = None;
        }
        Err(error) => menu.status = Some(format!("start failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::{save::WorldStore, steam::AuthenticatedUser, world::MapType};

    fn raw_input() -> egui::RawInput {
        raw_input_with_events(Vec::new())
    }

    fn raw_input_with_events(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 768.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn key_press(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn temp_store() -> SaveStore {
        SaveStore(WorldStore::new(
            std::env::temp_dir().join(format!("game-worlds-ui-test-{}", Uuid::new_v4())),
        ))
    }

    fn steam_user() -> SteamUser {
        SteamUser(AuthenticatedUser {
            steam_id: 42,
            display_name: "Dannie".to_owned(),
            token: "offline:42".to_owned(),
        })
    }

    #[test]
    fn layout_helpers_keep_action_columns_fixed() {
        let columns = WorldColumns::for_width(640.0);
        assert_eq!(
            columns.actions,
            START_BUTTON_WIDTH
                + ACTION_BUTTON_GAP
                + EDIT_BUTTON_WIDTH
                + ACTION_BUTTON_GAP
                + DELETE_BUTTON_WIDTH
        );
        assert!(columns.name >= 150.0);
        assert!(columns.map >= 100.0);

        let content_rect =
            egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(640.0, ROW_HEIGHT));
        let cells = column_rects(content_rect, columns);
        assert_eq!(cells.name.left(), content_rect.left());
        assert_eq!(cells.map.left(), cells.name.right() + COLUMN_GAP);
        assert_eq!(cells.actions.left(), cells.map.right() + COLUMN_GAP);

        let zero_width = cell_rect(content_rect, 24.0, -10.0);
        assert_eq!(zero_width.width(), 0.0);
        assert_eq!(HeaderCell::new("World").text, "World");
    }

    #[test]
    fn refresh_worlds_handles_success_and_list_errors() {
        let store = temp_store();
        let mut menu = MenuState::default();
        let first = store
            .0
            .create_world("Beta", Some(42))
            .expect("world should create");
        let second = store
            .0
            .create_world("Alpha", Some(42))
            .expect("world should create");

        refresh_worlds(&mut menu, &store);

        assert_eq!(menu.worlds.len(), 2);
        assert!(menu.status.is_none());
        assert!(menu.worlds.iter().any(|world| world.id == first.id));
        assert!(menu.worlds.iter().any(|world| world.id == second.id));

        let bad_root = std::env::temp_dir().join(format!("game-worlds-ui-file-{}", Uuid::new_v4()));
        fs::write(&bad_root, "not a directory").expect("file should write");
        let bad_store = SaveStore(WorldStore::new(&bad_root));
        refresh_worlds(&mut menu, &bad_store);

        assert!(menu.worlds.is_empty());
        assert!(
            menu.status
                .expect("status should exist")
                .contains("world list failed")
        );

        let _ = fs::remove_dir_all(store.0.root());
        let _ = fs::remove_file(bad_root);
    }

    #[test]
    fn start_singleplayer_updates_runtime_or_reports_load_error() {
        let store = temp_store();
        let user = steam_user();
        let save = store
            .0
            .create_world("Local", Some(user.0.steam_id))
            .expect("world should create");
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();

        start_singleplayer(&mut menu, &mut runtime, &store, &user, save.id);

        assert_eq!(menu.screen, Screen::InGame);
        assert!(!menu.pause_open);
        assert!(!menu.chat_open);
        assert_eq!(runtime.active_world_id, Some(save.id));
        assert!(runtime.session.is_some());

        start_singleplayer(&mut menu, &mut runtime, &store, &user, Uuid::new_v4());

        assert!(
            menu.status
                .expect("status should exist")
                .contains("start failed")
        );

        let _ = fs::remove_dir_all(store.0.root());
    }

    #[test]
    fn create_world_from_dialog_persists_selected_map() {
        let store = temp_store();
        let user = steam_user();
        let mut menu = MenuState::default();
        let dialog = CreateWorldDialog {
            name: "Generated".to_owned(),
            map_kind: CreateWorldMapKind::Procedural,
            procedural_size: ProceduralMapSize::Small,
            seed: "1234".to_owned(),
            error: None,
            closing: false,
            confirmed: true,
        };

        create_world_from_dialog(dialog, &mut menu, &store, &user);

        assert!(menu.status.is_none());
        assert_eq!(menu.worlds.len(), 1);
        assert_eq!(
            menu.worlds[0].map,
            MapType::Procedural {
                seed: 1234,
                size: ProceduralMapSize::Small,
            }
        );

        let _ = fs::remove_dir_all(store.0.root());
    }

    #[test]
    fn rename_world_from_dialog_updates_name_only() {
        let store = temp_store();
        let save = store
            .0
            .create_world_with_map(
                "Original",
                Some(42),
                MapType::Procedural {
                    seed: 1234,
                    size: ProceduralMapSize::Large,
                },
            )
            .expect("world should create");
        let mut menu = MenuState::default();

        refresh_worlds(&mut menu, &store);
        let mut dialog = EditWorldDialog::new(&menu.worlds[0]);
        dialog.name = "Renamed".to_owned();

        rename_world_from_dialog(dialog, &mut menu, &store);

        assert!(menu.status.is_none());
        assert_eq!(menu.worlds[0].name, "Renamed");
        assert_eq!(menu.worlds[0].id, save.id);
        assert_eq!(
            menu.worlds[0].map,
            MapType::Procedural {
                seed: 1234,
                size: ProceduralMapSize::Large,
            }
        );

        let _ = fs::remove_dir_all(store.0.root());
    }

    #[test]
    fn escape_cancels_modal_or_returns_to_main_menu() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            screen: Screen::Worlds,
            create_world: Some(CreateWorldDialog::new()),
            ..Default::default()
        };

        let _ = ctx.run(
            raw_input_with_events(vec![key_press(egui::Key::Escape)]),
            |ctx| {
                handle_worlds_escape(ctx, &mut menu);
            },
        );

        let create_dialog = menu
            .create_world
            .expect("dialog should remain while closing");
        assert!(create_dialog.closing);
        assert!(!create_dialog.confirmed);
        assert_eq!(menu.screen, Screen::Worlds);

        let ctx = egui::Context::default();
        let mut menu = MenuState {
            screen: Screen::Worlds,
            ..Default::default()
        };

        let _ = ctx.run(
            raw_input_with_events(vec![key_press(egui::Key::Escape)]),
            |ctx| {
                handle_worlds_escape(ctx, &mut menu);
            },
        );

        assert_eq!(menu.screen, Screen::MainMenu);
    }

    #[test]
    fn worlds_ui_renders_empty_and_populated_tables() {
        let ctx = egui::Context::default();
        let store = temp_store();
        let user = steam_user();
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime::default();

        let _ = ctx.run(raw_input(), |ctx| {
            worlds_ui(ctx, &mut menu, &mut runtime, &store, &user);
        });

        store
            .0
            .create_world("Rendered", Some(user.0.steam_id))
            .expect("world should create");
        refresh_worlds(&mut menu, &store);
        assert_eq!(menu.worlds[0].map, MapType::Test);

        let _ = ctx.run(raw_input(), |ctx| {
            worlds_ui(ctx, &mut menu, &mut runtime, &store, &user);
        });

        assert!(table_height(&ctx) >= 180.0);

        let _ = fs::remove_dir_all(store.0.root());
    }
}
