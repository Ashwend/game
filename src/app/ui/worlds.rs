use anyhow::Context;
use bevy_egui::egui;
use uuid::Uuid;

use crate::{
    app::state::{ClientRuntime, ConfirmationDialog, MenuState, SaveStore, Screen, SteamUser},
    net::ClientSession,
};

use super::theme::{self, ButtonKind};

const INSET_FRAME_HORIZONTAL_PADDING: f32 = 28.0;
const ROW_HEIGHT: f32 = 60.0;
const ROW_HORIZONTAL_PADDING: f32 = 14.0;
const COLUMN_GAP: f32 = 18.0;
const START_BUTTON_WIDTH: f32 = 78.0;
const DELETE_BUTTON_WIDTH: f32 = 82.0;
const ACTION_BUTTON_GAP: f32 = 10.0;
const BUTTON_HEIGHT: f32 = 34.0;

pub(super) fn worlds_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
) {
    theme::screen_scrim(ctx, "worlds_scrim", 145);
    theme::anchored_panel(
        ctx,
        "worlds_panel",
        920.0,
        egui::Align2::CENTER_CENTER,
        [0.0, -8.0],
        |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::section("Singleplayer Worlds"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::compact_button(ui, "Back", ButtonKind::Secondary, 78.0).clicked() {
                        menu.screen = Screen::MainMenu;
                    }
                });
            });

            ui.add_space(16.0);
            theme::inset_frame().show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 38.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_sized(
                            [92.0, 34.0],
                            egui::Label::new(theme::field_label("New World")),
                        );
                        ui.add_sized([360.0, 34.0], theme::text_input(&mut menu.new_world_name));
                        if theme::compact_button(ui, "Create", ButtonKind::Primary, 92.0).clicked()
                        {
                            match store
                                .0
                                .create_world(&menu.new_world_name, Some(user.0.steam_id))
                            {
                                Ok(_) => {
                                    menu.new_world_name = "New World".to_owned();
                                    refresh_worlds(menu, store);
                                }
                                Err(error) => menu.status = Some(format!("create failed: {error}")),
                            }
                        }
                    },
                );
            });

            ui.add_space(14.0);
            draw_world_headers(ui);
            let table_height = table_height(ctx);
            draw_world_table(ui, menu, runtime, store, user, table_height);

            if let Some(status) = &menu.status {
                ui.add_space(10.0);
                ui.label(theme::status_text(status));
            }
        },
    );
}

fn table_height(ctx: &egui::Context) -> f32 {
    (ctx.content_rect().height() - 315.0).max(180.0)
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
            ui.vertical_centered(|ui| {
                ui.add_space((table_height * 0.5 - 14.0).max(24.0));
                ui.label(theme::muted("No worlds yet."));
            });
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
            HeaderCell::new("Seed"),
            HeaderCell::new("Admins"),
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
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(row_width, ROW_HEIGHT), egui::Sense::hover());
    let fill = if response.hovered() {
        egui::Color32::from_rgba_unmultiplied(12, 18, 24, 238)
    } else {
        egui::Color32::from_rgba_unmultiplied(7, 10, 14, 218)
    };
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
        cells.seed,
        world.seed.to_string(),
        theme::muted_text(),
        13.0,
        egui::FontFamily::Monospace,
    );
    draw_cell_text(
        ui,
        cells.admins,
        world.admin_count.to_string(),
        theme::text(),
        14.0,
        egui::FontFamily::Proportional,
    );

    let button_y = cells.actions.center().y;
    let start_rect = egui::Rect::from_min_size(
        egui::pos2(cells.actions.left(), button_y - BUTTON_HEIGHT * 0.5),
        egui::vec2(START_BUTTON_WIDTH, BUTTON_HEIGHT),
    );
    let delete_rect = egui::Rect::from_min_size(
        egui::pos2(
            start_rect.right() + ACTION_BUTTON_GAP,
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
    seed: egui::Rect,
    admins: egui::Rect,
    actions: egui::Rect,
}

fn draw_columns(
    ui: &egui::Ui,
    content_rect: egui::Rect,
    columns: WorldColumns,
    headers: [HeaderCell; 4],
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
        cells.seed,
        headers[1].text,
        egui::Color32::from_rgb(172, 190, 208),
        12.0,
        egui::FontFamily::Proportional,
    );
    draw_cell_text(
        ui,
        cells.admins,
        headers[2].text,
        egui::Color32::from_rgb(172, 190, 208),
        12.0,
        egui::FontFamily::Proportional,
    );
    draw_cell_text(
        ui,
        cells.actions,
        headers[3].text,
        egui::Color32::from_rgb(172, 190, 208),
        12.0,
        egui::FontFamily::Proportional,
    );
}

fn column_rects(content_rect: egui::Rect, columns: WorldColumns) -> ColumnRects {
    let mut x = content_rect.left();
    let name = cell_rect(content_rect, x, columns.name);
    x += columns.name + COLUMN_GAP;
    let seed = cell_rect(content_rect, x, columns.seed);
    x += columns.seed + COLUMN_GAP;
    let admins = cell_rect(content_rect, x, columns.admins);
    x += columns.admins + COLUMN_GAP;
    let actions = cell_rect(content_rect, x, columns.actions);

    ColumnRects {
        name,
        seed,
        admins,
        actions,
    }
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
    seed: f32,
    admins: f32,
    actions: f32,
}

impl WorldColumns {
    fn for_width(width: f32) -> Self {
        let actions = START_BUTTON_WIDTH + ACTION_BUTTON_GAP + DELETE_BUTTON_WIDTH;
        let admins = 64.0;
        let remaining = (width - actions - admins - COLUMN_GAP * 3.0).max(0.0);
        let seed = (remaining * 0.48)
            .clamp(180.0, 330.0)
            .min((remaining - 150.0).max(120.0));
        let name = (remaining - seed).max(150.0);

        Self {
            name,
            seed,
            admins,
            actions,
        }
    }
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
