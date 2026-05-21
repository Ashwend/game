use bevy_egui::egui;
use uuid::Uuid;

use crate::{
    app::state::{ConfirmationDialog, EditWorldDialog, MenuState, SaveStore, SteamUser},
    save::{CorruptedWorld, WorldSummary},
};

use super::super::theme::{self, ButtonKind};
use super::{
    BUTTON_HEIGHT, dialogs::open_create_world_dialog, session::start_singleplayer_in_background,
};

const INSET_FRAME_HORIZONTAL_PADDING: f32 = 28.0;
const ROW_HEIGHT: f32 = 60.0;
const ROW_HORIZONTAL_PADDING: f32 = 14.0;
const COLUMN_GAP: f32 = 18.0;
const START_BUTTON_WIDTH: f32 = 78.0;
const EDIT_BUTTON_WIDTH: f32 = 64.0;
const DELETE_BUTTON_WIDTH: f32 = 82.0;
const ACTION_BUTTON_GAP: f32 = 10.0;

/// Vertical budget for the worlds table inside the bounded worlds panel.
/// Uses the remaining `ui.available_height()` minus whatever the caller
/// still needs to render below (status line, padding), so the table grows
/// to fill the panel on tall windows and stays at least one row tall on
/// short ones.
pub(super) fn available_table_height(ui: &egui::Ui, reserve_below: f32) -> f32 {
    (ui.available_height() - reserve_below).max(180.0)
}

pub(super) fn draw_world_table(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    store: &SaveStore,
    user: &SteamUser,
    table_height: f32,
) {
    let table_outer_width = ui.available_width();
    theme::inset_frame().show(ui, |ui| {
        let table_content_width = (table_outer_width - INSET_FRAME_HORIZONTAL_PADDING).max(0.0);
        ui.set_width(table_content_width);
        ui.set_min_height(table_height);
        if menu.worlds.is_empty() && menu.corrupted_worlds.is_empty() {
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
                            draw_world_row(ui, menu, store, user, world, table_content_width);
                            ui.add_space(8.0);
                        }
                        let corrupted = menu.corrupted_worlds.clone();
                        for entry in corrupted {
                            draw_corrupted_world_row(ui, menu, entry, table_content_width);
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

    let starting_world = menu.world_start.as_ref().map(|attempt| attempt.world_id);
    let start_state = if starting_world == Some(world.id) {
        theme::ButtonState::Loading
    } else {
        theme::ButtonState::Ready
    };
    let start_response = theme::compact_button_in_rect_with_state(
        ui,
        ("world-start", world.id),
        start_rect,
        "Start",
        ButtonKind::Primary,
        start_state,
    );
    if start_response.clicked() && starting_world.is_none() {
        start_singleplayer_in_background(menu, store, user, world.id);
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

/// Padding reserved on the left of the name cell for the warning icon on
/// corrupted rows. Matches the icon size so the name text starts at the
/// same x as it would on a healthy row plus icon + small gap.
const CORRUPTED_NAME_ICON_GAP: f32 = 24.0;

fn draw_corrupted_world_row(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    entry: CorruptedWorld,
    row_outer_width: f32,
) {
    let row_width = row_outer_width.max(0.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(row_width, ROW_HEIGHT), egui::Sense::hover());
    // Slightly warmer fill + warning-tinted stroke so the row reads as
    // "needs attention" without being alarming.
    let fill = egui::Color32::from_rgba_unmultiplied(28, 16, 12, 220);
    ui.painter().rect(
        rect,
        5,
        fill,
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(190, 110, 70, 130)),
        egui::StrokeKind::Inside,
    );

    let content_rect = rect.shrink2(egui::vec2(ROW_HORIZONTAL_PADDING, 0.0));
    let columns = WorldColumns::for_width(content_rect.width());
    let cells = column_rects(content_rect, columns);

    let row_id = ui.id().with(("world-corrupted", entry.file_name.clone()));
    draw_warning_icon(ui, cells.name, row_id, &entry);

    let name_text_rect = cells
        .name
        .with_min_x(cells.name.left() + CORRUPTED_NAME_ICON_GAP);
    draw_cell_text(
        ui,
        name_text_rect,
        entry.display_name(),
        egui::Color32::from_rgb(244, 210, 192),
        14.0,
        egui::FontFamily::Proportional,
    );
    draw_cell_text(
        ui,
        cells.map,
        "Unknown",
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

    paint_disabled_button(ui, start_rect, "Start");
    paint_disabled_button(ui, edit_rect, "Edit");

    if let Some(world_id) = entry.id {
        delete_corrupted_button(ui, menu, world_id, &entry, delete_rect);
    } else {
        // No recoverable id — we can't route to `WorldStore::delete_world`
        // (which keys off Uuid). Render a disabled Delete so the row is
        // still visually consistent, and put the reason in the tooltip.
        paint_disabled_button(ui, delete_rect, "Delete");
    }
}

fn draw_warning_icon(
    ui: &mut egui::Ui,
    name_cell: egui::Rect,
    row_id: egui::Id,
    entry: &CorruptedWorld,
) {
    let icon_size = 18.0;
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(
            name_cell.left() + icon_size * 0.5 + 2.0,
            name_cell.center().y,
        ),
        egui::vec2(icon_size, icon_size),
    );
    let response = ui
        .interact(icon_rect, row_id.with("warning"), egui::Sense::hover())
        .on_hover_cursor(egui::CursorIcon::Help);
    let color = egui::Color32::from_rgb(244, 178, 96);
    ui.painter().text(
        icon_rect.center(),
        egui::Align2::CENTER_CENTER,
        "\u{26A0}", // ⚠ warning sign
        egui::FontId::new(16.0, egui::FontFamily::Proportional),
        color,
    );
    let title = entry.display_name();
    let body = format!(
        "This save couldn't be loaded.\nFile: {file}\n\n{error}",
        file = entry.file_name,
        error = entry.error,
    );
    let _ = theme::wow_tooltip(response, title, &body);
}

fn delete_corrupted_button(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    world_id: Uuid,
    entry: &CorruptedWorld,
    rect: egui::Rect,
) {
    if theme::compact_button_in_rect(
        ui,
        ("world-corrupted-delete", world_id),
        rect,
        "Delete",
        ButtonKind::Danger,
    )
    .clicked()
    {
        menu.confirmation = Some(ConfirmationDialog::delete_world(
            world_id,
            entry.display_name(),
        ));
    }
}

/// Paints a flat, non-interactive button so disabled actions on corrupted
/// rows still line up with the action column. Sense is hover-only so the
/// click never fires and the cursor stays at default.
fn paint_disabled_button(ui: &mut egui::Ui, rect: egui::Rect, label: &str) {
    ui.allocate_rect(rect, egui::Sense::hover());
    let fill = egui::Color32::from_rgba_unmultiplied(20, 24, 30, 200);
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(70, 80, 92, 140));
    ui.painter()
        .rect(rect, 4, fill, stroke, egui::StrokeKind::Inside);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        egui::Color32::from_rgba_unmultiplied(150, 160, 175, 200),
    );
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
