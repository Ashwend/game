use bevy_egui::egui;

use crate::{
    app::state::{
        ClientRuntime, ConfirmationDialog, EditWorldDialog, MenuState, SaveStore, SteamUser,
    },
    save::WorldSummary,
};

use super::super::theme::{self, ButtonKind};
use super::{BUTTON_HEIGHT, dialogs::open_create_world_dialog, session::start_singleplayer};

const INSET_FRAME_HORIZONTAL_PADDING: f32 = 28.0;
const ROW_HEIGHT: f32 = 60.0;
const ROW_HORIZONTAL_PADDING: f32 = 14.0;
const COLUMN_GAP: f32 = 18.0;
const START_BUTTON_WIDTH: f32 = 78.0;
const EDIT_BUTTON_WIDTH: f32 = 64.0;
const DELETE_BUTTON_WIDTH: f32 = 82.0;
const ACTION_BUTTON_GAP: f32 = 10.0;

pub(super) fn table_height(ctx: &egui::Context) -> f32 {
    (ctx.content_rect().height() - 240.0).max(180.0)
}

pub(super) fn draw_world_table(
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

pub(super) fn draw_world_headers(ui: &mut egui::Ui) {
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
    world: WorldSummary,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
