//! Toggle-to-view world-map overlay.
//!
//! Drawn on top of the game (translucent backdrop, so the scene stays visible)
//! while [`crate::app::state::MenuState::world_map_open`] is set. The biome
//! texture and the player's own markers come from [`WorldMapState`]; the
//! coordinate grid, axis labels, marker pins, and the player facing arrow are
//! drawn here per-frame from the local camera so they stay crisp at any
//! resolution.
//!
//! Unlike a read-only overlay this one is *interactive*: the player right-clicks
//! the map to drop a marker, left-clicks a marker to open a small name/delete
//! popup, and hovers a marker to see its label. Every mutation goes to the
//! server through [`crate::protocol::WorldMapMarkerCommand`]; the server owns
//! the marker store and pushes the updated list back. See
//! [`crate::app::systems::world_map`] for the data + input side.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::{
    app::state::{
        ClientRuntime, ConfirmationDialog, ErrorToastSink, MenuState, TextPrompt, TextPromptKind,
        WorldMapState, WorldMapUiState,
    },
    protocol::{ClientMessage, WorldMapMarker, WorldMapMarkerCommand},
};

use super::theme;

/// Grid spacing in world metres (two chunks). Lines at this cadence keep the
/// map readable without drowning it in gridlines on the large map.
const GRID_STEP_M: f32 = 128.0;

/// Zoomed-all-the-way-out: the whole world fits the map square.
const MIN_ZOOM: f32 = 1.0;
/// Closest zoom-in. Past this the biome raster (256 px) gets too soft to read.
const MAX_ZOOM: f32 = 6.0;
/// Wheel-zoom sensitivity: `zoom *= exp(scroll * K)`. Tuned for a gentle few
/// percent per notch; the per-frame factor is additionally clamped below.
const ZOOM_SCROLL_K: f32 = 0.004;

type Bounds = (f32, f32, f32, f32);

/// The square world region currently shown in the map, derived from the
/// pan/zoom state. `span` is the side length in metres; `(min_x, min_z)` is the
/// north-west corner.
struct MapView {
    min_x: f32,
    min_z: f32,
    span: f32,
}

impl MapView {
    fn center(&self) -> (f32, f32) {
        (self.min_x + self.span * 0.5, self.min_z + self.span * 0.5)
    }
}

/// Resolve the visible world region from the pan/zoom state, clamped so the
/// view never spills outside the world. At zoom 1 the clamp pins the centre to
/// the world centre (the whole world is shown), so panning only bites once
/// zoomed in.
fn compute_view(bounds: Bounds, ui_state: &WorldMapUiState) -> MapView {
    let (min_x, min_z, max_x, max_z) = bounds;
    let full_span = (max_x - min_x).max(1.0);
    let zoom = ui_state.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    let span = full_span / zoom;
    let half = span * 0.5;
    let (cx, cz) = ui_state
        .center
        .unwrap_or(((min_x + max_x) * 0.5, (min_z + max_z) * 0.5));
    // `min + half <= max - half` holds for zoom >= 1, so the clamp is valid
    // (degenerate to a single point at zoom 1).
    let cx = cx.clamp(min_x + half, max_x - half);
    let cz = cz.clamp(min_z + half, max_z - half);
    MapView {
        min_x: cx - half,
        min_z: cz - half,
        span,
    }
}

/// The texture UV sub-rect that the current view selects out of the full-world
/// raster, so a zoomed view samples just its slice of the biome image.
fn view_uv(bounds: Bounds, view: &MapView) -> egui::Rect {
    let (min_x, min_z, max_x, _) = bounds;
    let full = (max_x - min_x).max(1.0);
    let u0 = ((view.min_x - min_x) / full).clamp(0.0, 1.0);
    let v0 = ((view.min_z - min_z) / full).clamp(0.0, 1.0);
    let d = (view.span / full).clamp(0.0, 1.0);
    egui::Rect::from_min_max(
        egui::pos2(u0, v0),
        egui::pos2((u0 + d).min(1.0), (v0 + d).min(1.0)),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn world_map_ui(
    ctx: &egui::Context,
    world_map: &WorldMapState,
    ui_state: &mut WorldMapUiState,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    player: Option<GlobalTransform>,
) {
    let screen = ctx.content_rect();

    // Square map region, centred, with a margin on all sides.
    let margin = (screen.width().min(screen.height()) * 0.07).clamp(28.0, 110.0);
    let side = screen.width().min(screen.height()) - margin * 2.0;
    let map_rect = egui::Rect::from_center_size(screen.center(), egui::vec2(side, side));
    let bounds = world_map.bounds();

    // Freeze the map's own pan/zoom/marker input while a modal sits on top of it
    // (the marker-name prompt or the delete-confirm): those capture clicks via
    // their backdrop, but the wheel is read straight from raw input, so without
    // this guard scrolling would still zoom the map behind the dialog.
    let interactive = menu.confirmation.is_none() && menu.text_prompt.is_none();

    // The whole overlay lives in one foreground Area so its background can
    // sense clicks + drags (right-click adds a marker, drag pans, left-click on
    // empty space closes any open popup). Markers are interacted with inside
    // it, on top. Returns the hovered marker's tooltip data, if any.
    let hover_tip = egui::Area::new(egui::Id::new("world_map_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let painter = ui.painter().clone();
            // A clip of the painter for everything that lives *inside* the map
            // square (terrain, grid, pins, player), so a zoomed/panned view and
            // edge pins never spill past the frame.
            let map_painter = painter.with_clip_rect(map_rect);

            // Dimmed translucent backdrop: the game stays visible behind it.
            painter.rect_filled(
                screen,
                0,
                egui::Color32::from_rgba_unmultiplied(6, 8, 12, 180),
            );
            // Panel behind the map (shows through while the texture loads).
            painter.rect_filled(map_rect, 8, egui::Color32::from_rgb(18, 22, 28));

            // Background interaction covers the whole screen so a click in the
            // dimmed margin also dismisses an open popup. Allocated before the
            // markers so the markers sit on top for hit-testing. `click_and_drag`
            // so the same widget drives both right-click-add and drag-to-pan.
            let bg = ui.allocate_rect(screen, egui::Sense::click_and_drag());

            // The hovered marker's pin position + label, returned from the Area
            // so a self-rendered tooltip can be drawn on top afterwards. egui's
            // built-in `on_hover_text` is suppressed here because it requires
            // the pointer to be still with zero scroll delta, and the map eats
            // the wheel for zoom, so its scroll timer keeps the tooltip hidden.
            let mut hover_tip: Option<(egui::Pos2, String)> = None;

            if let Some(bounds) = bounds {
                let full_span = (bounds.2 - bounds.0).max(1.0);

                // --- viewport interactions, applied this frame (skipped while a
                // modal sits on top of the map) ---
                // Wheel zoom, anchored so the world point under the cursor stays
                // under the cursor.
                let scroll = if interactive {
                    ui.input(|i| i.smooth_scroll_delta.y)
                } else {
                    0.0
                };
                if scroll != 0.0
                    && let Some(cursor) = ui.input(|i| i.pointer.hover_pos())
                    && map_rect.contains(cursor)
                {
                    let view = compute_view(bounds, ui_state);
                    let (ax, az) = map_to_world(map_rect, &view, cursor);
                    let factor = (scroll * ZOOM_SCROLL_K).exp().clamp(0.5, 2.0);
                    let new_zoom = (ui_state.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
                    let new_span = full_span / new_zoom;
                    let u = (cursor.x - map_rect.left()) / map_rect.width();
                    let v = (cursor.y - map_rect.top()) / map_rect.height();
                    ui_state.zoom = new_zoom;
                    ui_state.center = Some((ax + (0.5 - u) * new_span, az + (0.5 - v) * new_span));
                }
                // Drag to pan: grabbing the map drags the world under the cursor.
                if interactive && bg.dragged() {
                    let view = compute_view(bounds, ui_state);
                    let world_per_px = view.span / map_rect.width();
                    let (cx, cz) = view.center();
                    let delta = bg.drag_delta();
                    ui_state.center =
                        Some((cx - delta.x * world_per_px, cz - delta.y * world_per_px));
                }

                // --- draw with the resolved view ---
                let view = compute_view(bounds, ui_state);
                if let Some(texture) = world_map.texture() {
                    map_painter.image(
                        texture,
                        map_rect,
                        view_uv(bounds, &view),
                        egui::Color32::WHITE,
                    );
                } else {
                    painter.text(
                        map_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Loading map...",
                        egui::FontId::proportional(18.0),
                        egui::Color32::from_gray(180),
                    );
                }
                draw_grid(&map_painter, map_rect, &view);

                // Markers: draw a pin per visible marker, sense hover/click.
                // Off-view pins are skipped entirely (no draw, no interaction).
                // `any_marker_hovered` lets the background actions ignore a
                // click that landed on a pin, regardless of egui's overlap
                // tie-break.
                let mut clicked_marker = None;
                let mut any_marker_hovered = false;
                for marker in world_map.markers() {
                    let pos = world_to_map(map_rect, &view, marker.x, marker.z);
                    if !map_rect.contains(pos) {
                        continue;
                    }
                    let selected = ui_state.selected_marker == Some(marker.id);
                    // Hover does not restyle the pin (only the open popup does, a
                    // slight enlarge + brighten); the name surfaces through the
                    // tooltip below instead.
                    draw_marker_pin(&map_painter, pos, selected);
                    if !interactive {
                        continue;
                    }
                    let hit = pin_hit_rect(pos);
                    let resp = ui.interact(
                        hit,
                        ui.id().with(("world_map_marker", marker.id)),
                        egui::Sense::click(),
                    );
                    let hovered = resp.hovered();
                    if resp.clicked() {
                        clicked_marker = Some(marker.id);
                    }
                    any_marker_hovered |= hovered;
                    // Tooltip on hover, but not for the active marker (its popup
                    // already names it).
                    if hovered && !selected {
                        hover_tip = Some((pos, marker_tooltip(marker)));
                    }
                }

                if let Some(transform) = player {
                    let pos = world_to_map(
                        map_rect,
                        &view,
                        transform.translation().x,
                        transform.translation().z,
                    );
                    if map_rect.contains(pos) {
                        draw_player(&map_painter, pos, transform);
                    }
                }

                // Resolve interactions. Clicking a marker toggles its popup:
                // clicking the active one closes it, clicking another switches
                // to it. A plain left-click on empty space closes any open
                // popup. A drag is not a click, so panning never deselects.
                if let Some(id) = clicked_marker {
                    ui_state.selected_marker = if ui_state.selected_marker == Some(id) {
                        None
                    } else {
                        Some(id)
                    };
                } else if bg.clicked() && !any_marker_hovered {
                    ui_state.selected_marker = None;
                }
                // Right-click on the terrain drops a new (unnamed) marker.
                if bg.secondary_clicked()
                    && !any_marker_hovered
                    && let Some(pos) = bg.interact_pointer_pos()
                    && map_rect.contains(pos)
                {
                    let (wx, wz) = map_to_world(map_rect, &view, pos);
                    send_marker_command(
                        runtime,
                        error_toasts,
                        WorldMapMarkerCommand::Add { x: wx, z: wz },
                    );
                    ui_state.selected_marker = None;
                }

                // Grab/grabbing cursor over the pannable map (but not over a pin).
                if bg.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                } else if bg.hovered() && !any_marker_hovered {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
            } else {
                painter.text(
                    map_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Loading map...",
                    egui::FontId::proportional(18.0),
                    egui::Color32::from_gray(180),
                );
            }

            // Frame + title (unclipped, so the stroke + title sit on the edge).
            painter.rect_stroke(
                map_rect,
                8,
                egui::Stroke::new(1.5, egui::Color32::from_gray(96)),
                egui::StrokeKind::Inside,
            );
            painter.text(
                egui::pos2(map_rect.center().x, map_rect.top() - 8.0),
                egui::Align2::CENTER_BOTTOM,
                "World Map",
                egui::FontId::proportional(17.0),
                egui::Color32::from_gray(222),
            );
            if let Some(transform) = player {
                let translation = transform.translation();
                painter.text(
                    egui::pos2(map_rect.center().x, map_rect.bottom() + 6.0),
                    egui::Align2::CENTER_TOP,
                    format!(
                        "X {}   Z {}   (metres)",
                        translation.x.round() as i32,
                        translation.z.round() as i32
                    ),
                    egui::FontId::monospace(12.0),
                    egui::Color32::from_gray(150),
                );
            }
            // Discoverability hint for the map controls, in two short lines.
            painter.text(
                egui::pos2(map_rect.center().x, map_rect.bottom() + 24.0),
                egui::Align2::CENTER_TOP,
                "Scroll to zoom  •  drag to pan  •  right-click to add a marker",
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(120),
            );
            painter.text(
                egui::pos2(map_rect.center().x, map_rect.bottom() + 39.0),
                egui::Align2::CENTER_TOP,
                "Click a marker to name or delete it",
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(120),
            );

            hover_tip
        })
        .inner;

    // Self-rendered hover tooltip for a marker. Drawn as its own tooltip-order
    // Area that follows the cursor (like egui's built-in tooltip), but without
    // the scroll-gated timing that hides the built-in one while the map eats the
    // wheel for zoom. The label never wraps, so long names stay on one line.
    if let Some((pin, label)) = hover_tip {
        let anchor = ctx.input(|i| i.pointer.hover_pos()).unwrap_or(pin);
        let mut pos = anchor + egui::vec2(14.0, 16.0);
        // Keep it on screen; flip above-left near the right/bottom edges.
        if pos.x > screen.right() - 220.0 {
            pos.x = anchor.x - 14.0 - 200.0;
        }
        pos.y = pos.y.min(screen.bottom() - 40.0).max(screen.top() + 4.0);
        egui::Area::new(egui::Id::new("world_map_marker_tooltip"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.add(egui::Label::new(label).wrap_mode(egui::TextWrapMode::Extend));
                });
            });
    }

    // Action popup for the selected marker, drawn as its own foreground Area
    // (created after the overlay, so it sits on top and swallows its own
    // clicks). Resolved here, after the overlay's borrows are released. Hidden
    // when the pin is panned off the visible map.
    if let (Some(id), Some(bounds)) = (ui_state.selected_marker, bounds) {
        if let Some(marker) = world_map.markers().iter().find(|m| m.id == id) {
            let view = compute_view(bounds, ui_state);
            let pin = world_to_map(map_rect, &view, marker.x, marker.z);
            if map_rect.contains(pin) {
                marker_popup(ctx, ui_state, menu, id, &marker.name, pin);
            }
        } else {
            // The marker vanished (deleted on another client / refresh).
            ui_state.selected_marker = None;
        }
    }
}

/// The small name/delete popup anchored next to a marker pin. Neither button
/// touches the network here: Name opens the rename text prompt, Delete opens
/// the shared confirm modal (which arms the actual remove command).
fn marker_popup(
    ctx: &egui::Context,
    ui_state: &mut WorldMapUiState,
    menu: &mut MenuState,
    id: u32,
    current_name: &str,
    pin: egui::Pos2,
) {
    // Offset up-right of the pin so it doesn't cover the point itself, then
    // nudge it back on-screen if it would spill off the right/top edge.
    let screen = ctx.content_rect();
    let mut pos = pin + egui::vec2(12.0, -16.0);
    pos.x = pos.x.min(screen.right() - 180.0);
    pos.y = pos.y.max(screen.top() + 8.0);

    // Per-marker Area id so switching the selection to a different marker spins
    // up a fresh popup rather than reusing the previous one's cached state.
    egui::Area::new(egui::Id::new(("world_map_marker_popup", id)))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(168.0);
                let heading = if current_name.is_empty() {
                    "Unnamed marker".to_owned()
                } else {
                    current_name.to_owned()
                };
                ui.label(theme::section(&heading));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::compact_button(ui, "Name", theme::ButtonKind::Primary, 72.0).clicked()
                    {
                        // Open the rename modal, pre-filled with the current
                        // label. The text prompt sits on top of the still-open
                        // map and submits the Rename command itself.
                        let mut prompt = TextPrompt::new(TextPromptKind::NameWorldMapMarker { id });
                        prompt.input = current_name.to_owned();
                        menu.text_prompt = Some(prompt);
                        ui_state.selected_marker = None;
                    }
                    if theme::compact_button(ui, "Delete", theme::ButtonKind::Danger, 72.0)
                        .clicked()
                    {
                        // Route through the shared confirm modal; on confirm it
                        // arms `world_map_delete_pending`, which the input system
                        // turns into the server remove command.
                        menu.confirmation = Some(ConfirmationDialog::delete_world_map_marker(
                            id,
                            current_name,
                        ));
                        ui_state.selected_marker = None;
                    }
                });
            });
        });
}

/// Send a marker mutation to the server, surfacing a transport error as a toast.
fn send_marker_command(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    command: WorldMapMarkerCommand,
) {
    let Some(session) = runtime.session.as_mut() else {
        return;
    };
    if let Err(error) = session.send(ClientMessage::WorldMapMarker(command)) {
        error_toasts.push_error(format!("couldn't update marker: {error}"));
    }
}

/// Tooltip text for a marker: its label, or a hint when it's still unnamed.
fn marker_tooltip(marker: &WorldMapMarker) -> String {
    if marker.name.is_empty() {
        "Unnamed marker (click to name)".to_owned()
    } else {
        marker.name.clone()
    }
}

/// Map a world x/z onto the map rect for the current view. The texture is
/// rastered with the same orientation (row 0 = min_z), so markers and grid line
/// up with the image. Not clamped: an off-view point maps outside `rect` so the
/// caller can cull it.
fn world_to_map(rect: egui::Rect, view: &MapView, world_x: f32, world_z: f32) -> egui::Pos2 {
    let u = (world_x - view.min_x) / view.span;
    let v = (world_z - view.min_z) / view.span;
    egui::pos2(
        rect.left() + u * rect.width(),
        rect.top() + v * rect.height(),
    )
}

/// Inverse of [`world_to_map`]: a point on the map rect back to world x/z.
fn map_to_world(rect: egui::Rect, view: &MapView, pos: egui::Pos2) -> (f32, f32) {
    let u = (pos.x - rect.left()) / rect.width();
    let v = (pos.y - rect.top()) / rect.height();
    (view.min_x + u * view.span, view.min_z + v * view.span)
}

/// Clickable bounding box for a pin whose tip is at `tip`. Covers the head
/// above the tip plus a little slack so it's comfortable to click and hover.
/// Sized to the larger pin drawn by [`draw_marker_pin`].
fn pin_hit_rect(tip: egui::Pos2) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(tip.x - 11.0, tip.y - 30.0),
        egui::pos2(tip.x + 11.0, tip.y + 5.0),
    )
}

fn draw_grid(painter: &egui::Painter, rect: egui::Rect, view: &MapView) {
    let max_x = view.min_x + view.span;
    let max_z = view.min_z + view.span;
    // Higher-contrast lines than the original: the minor grid is a clearly
    // visible light line, the x=0 / z=0 axes a thick cyan-blue. A thin dark
    // halo is drawn under each minor line so it reads over pale biomes too.
    let minor = egui::Color32::from_rgba_unmultiplied(244, 247, 255, 120);
    let minor_halo = egui::Color32::from_black_alpha(60);
    let axis = egui::Color32::from_rgba_unmultiplied(120, 198, 255, 235);
    let font = egui::FontId::monospace(11.0);

    // Draw one grid line: a dark halo under a light minor line so it stays
    // legible over pale biomes, or a single thick stroke for an axis.
    let grid_line = |a: egui::Pos2, b: egui::Pos2, is_axis: bool| {
        if is_axis {
            painter.line_segment([a, b], egui::Stroke::new(2.0, axis));
        } else {
            painter.line_segment([a, b], egui::Stroke::new(2.4, minor_halo));
            painter.line_segment([a, b], egui::Stroke::new(1.2, minor));
        }
    };

    // Iterate only the lines that fall inside the current view, so zooming in
    // doesn't walk the whole world. The line through x = 0 / z = 0 is the axis.
    let mut grid_x = (view.min_x / GRID_STEP_M).ceil() * GRID_STEP_M;
    while grid_x <= max_x {
        let top = world_to_map(rect, view, grid_x, view.min_z);
        let bottom = world_to_map(rect, view, grid_x, max_z);
        grid_line(top, bottom, grid_x.abs() < 0.5);
        grid_label(
            painter,
            egui::pos2(top.x, rect.top() + 3.0),
            egui::Align2::CENTER_TOP,
            grid_x as i32,
            &font,
        );
        grid_x += GRID_STEP_M;
    }

    let mut grid_z = (view.min_z / GRID_STEP_M).ceil() * GRID_STEP_M;
    while grid_z <= max_z {
        let left = world_to_map(rect, view, view.min_x, grid_z);
        let right = world_to_map(rect, view, max_x, grid_z);
        grid_line(left, right, grid_z.abs() < 0.5);
        grid_label(
            painter,
            egui::pos2(rect.left() + 3.0, left.y),
            egui::Align2::LEFT_CENTER,
            grid_z as i32,
            &font,
        );
        grid_z += GRID_STEP_M;
    }
}

/// A grid coordinate label, drawn with a dark drop shadow so it reads against
/// any biome colour underneath.
fn grid_label(
    painter: &egui::Painter,
    pos: egui::Pos2,
    align: egui::Align2,
    value: i32,
    font: &egui::FontId,
) {
    let text = value.to_string();
    painter.text(
        pos + egui::vec2(1.0, 1.0),
        align,
        &text,
        font.clone(),
        egui::Color32::from_black_alpha(190),
    );
    painter.text(
        pos,
        align,
        &text,
        font.clone(),
        egui::Color32::from_rgb(226, 233, 242),
    );
}

/// Draw a teardrop pin whose tip sits exactly on the marker's map position.
/// `highlight` brightens and enlarges it slightly while the marker's popup is
/// open. Hover deliberately does NOT highlight (the name shows as a tooltip
/// instead), so a pin stays visually stable as the cursor passes over it.
fn draw_marker_pin(painter: &egui::Painter, tip: egui::Pos2, highlight: bool) {
    let head_r = if highlight { 9.5 } else { 8.0 };
    let head_offset = if highlight { 21.0 } else { 18.0 };
    let head = egui::pos2(tip.x, tip.y - head_offset);
    let fill = if highlight {
        egui::Color32::from_rgb(255, 224, 140)
    } else {
        egui::Color32::from_rgb(245, 196, 90)
    };
    let outline = egui::Color32::from_rgb(60, 40, 12);

    // Soft contact shadow under the tip so the pin reads as standing on the map.
    painter.circle_filled(
        egui::pos2(tip.x, tip.y + 1.0),
        head_r * 0.5,
        egui::Color32::from_black_alpha(70),
    );
    // Tail triangle from the head down to the tip.
    let tail = vec![
        tip,
        egui::pos2(head.x - head_r * 0.82, head.y + head_r * 0.55),
        egui::pos2(head.x + head_r * 0.82, head.y + head_r * 0.55),
    ];
    painter.add(egui::Shape::convex_polygon(
        tail,
        fill,
        egui::Stroke::new(1.0, outline),
    ));
    // Head circle on top, hiding the triangle's flat edge into a teardrop.
    painter.circle_filled(head, head_r, fill);
    painter.circle_stroke(head, head_r, egui::Stroke::new(1.2, outline));
    // Inner dot.
    painter.circle_filled(head, head_r * 0.42, egui::Color32::from_rgb(70, 46, 14));
    // No extra ring when active: the slight enlarge + brighter fill above is the
    // only "selected" feedback.
}

/// Draw the player as a single blue heading triangle centred on `pos`. The
/// triangle alone marks both position (its centroid) and facing (its tip), so
/// there's no dot or ring, just the arrow.
fn draw_player(painter: &egui::Painter, pos: egui::Pos2, transform: GlobalTransform) {
    // Camera forward projected onto the ground plane gives the heading; the
    // map uses +x right / +z down, the same axes, so no flip is needed.
    let forward = transform.forward();
    let mut dir = egui::vec2(forward.x, forward.z);
    if dir.length() < 1e-3 {
        dir = egui::vec2(0.0, -1.0);
    }
    let dir = dir.normalized();
    let perp = egui::vec2(-dir.y, dir.x);
    let tip = pos + dir * 11.0;
    let left = pos - dir * 7.0 + perp * 7.0;
    let right = pos - dir * 7.0 - perp * 7.0;
    painter.add(egui::Shape::convex_polygon(
        vec![tip, left, right],
        egui::Color32::from_rgb(96, 170, 255),
        egui::Stroke::new(1.5, egui::Color32::from_rgb(12, 30, 58)),
    ));
}
