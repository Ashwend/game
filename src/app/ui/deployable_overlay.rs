//! Floating nameplate + health bar above damaged placed structures.
//!
//! Spec from the design pass: only show the label when the player
//!  - is **looking at** the structure (camera-forward cone test),
//!  - is **near** it (≤ 6 m from the structure's centre),
//!  - the structure is **not at full health**.
//!
//! Falls back to silence in every other case so undamaged props don't
//! clutter the screen. Same screen-projection / fade-out / viewport-
//! clamp pattern as [`super::peer_overlay`] — extracted helpers stay
//! local for now since deployable labels are simpler (no chat bubble,
//! no voice indicator).

use bevy::prelude::*;
use bevy_egui::egui;

use crate::{
    app::scene::NetworkDeployedEntity, items::DeployableKind, protocol::DeployedEntityState,
};

/// Hard cutoff distance for the nameplate. Past this the label hides
/// entirely. Matches the conversational range the peer overlay uses
/// so the two UIs feel like the same family of feedback.
const DEPLOYABLE_DRAW_DISTANCE_M: f32 = 6.0;
/// Distance at which the alpha starts ramping down so the label
/// dissolves softly over the last metre.
const DEPLOYABLE_FADE_START_M: f32 = 5.0;
/// Cosine of the half-angle of the "looking at" cone. ~25° each side
/// of the camera forward — generous enough that you don't have to
/// pixel-aim, tight enough that you can't see labels on structures
/// off to your peripheral vision.
const DEPLOYABLE_LOOK_CONE_COS: f32 = 0.91;
/// Inset (logical pixels) before the viewport edge.
const DEPLOYABLE_VIEWPORT_INSET_PX: f32 = 12.0;
/// Height (world space) above the structure's anchor where the
/// nameplate floats. Picked to clear the workbench tabletop (~1 m) and
/// the furnace chimney (~1.6 m) by a comfortable margin.
const NAMEPLATE_TOP_CLEARANCE_M: f32 = 1.9;

const NAMETAG_WIDTH: f32 = 144.0;
const NAMETAG_HEIGHT: f32 = 32.0;
const HEALTH_BAR_HEIGHT: f32 = 4.0;

pub(crate) struct DeployableOverlay<'world> {
    pub(crate) camera: Option<(&'world Camera, GlobalTransform)>,
    pub(crate) entries: Vec<DeployableOverlayEntry<'world>>,
}

pub(crate) struct DeployableOverlayEntry<'world> {
    pub(crate) anchor_world: Vec3,
    pub(crate) state: &'world DeployedEntityState,
}

pub(super) fn deployable_overlay_ui(ctx: &egui::Context, overlay: DeployableOverlay<'_>) {
    let Some((camera, camera_transform)) = overlay.camera else {
        return;
    };
    let camera_forward = camera_transform.forward().as_vec3();
    let camera_origin = camera_transform.translation();
    let visible_rect = ctx.content_rect().shrink(DEPLOYABLE_VIEWPORT_INSET_PX);

    for entry in overlay.entries {
        // Hide at full health — undamaged props don't need a label.
        if entry.state.health >= entry.state.max_health {
            continue;
        }
        let to_anchor = entry.anchor_world - camera_origin;
        let distance = to_anchor.length();
        if distance > DEPLOYABLE_DRAW_DISTANCE_M {
            continue;
        }
        // Looking-at test: dot the unit ray to the structure with
        // camera forward. Below the cone cosine → not centred enough
        // to be considered "the player is looking at it."
        let cosine = if distance > 1e-3 {
            to_anchor.dot(camera_forward) / distance
        } else {
            1.0
        };
        if cosine < DEPLOYABLE_LOOK_CONE_COS {
            continue;
        }

        let Ok(screen) = camera.world_to_viewport(&camera_transform, entry.anchor_world) else {
            continue;
        };
        if !visible_rect.contains(egui::pos2(screen.x, screen.y)) {
            continue;
        }
        draw_deployable_label(ctx, screen, distance, entry.state);
    }
}

fn draw_deployable_label(
    ctx: &egui::Context,
    screen: Vec2,
    distance: f32,
    state: &DeployedEntityState,
) {
    let id = egui::Id::new(("deployable_overlay", state.id));
    let fade = distance_fade(distance);

    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .interactable(false)
        .movable(false)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(screen.x, screen.y))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                nameplate(ui, state, fade);
            });
        });
}

fn nameplate(ui: &mut egui::Ui, state: &DeployedEntityState, fade: f32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(NAMETAG_WIDTH, NAMETAG_HEIGHT),
        egui::Sense::hover(),
    );

    let bg_alpha = scaled(186, fade);
    let stroke_alpha = scaled(120, fade);
    ui.painter().rect(
        rect,
        4,
        egui::Color32::from_rgba_unmultiplied(8, 10, 14, bg_alpha),
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 138, 160, stroke_alpha),
        ),
        egui::StrokeKind::Inside,
    );

    let label = kind_label(state.kind);
    let name_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(8.0, 3.0),
        egui::pos2(rect.right() - 8.0, rect.top() + 18.0),
    );
    ui.painter().text(
        name_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
        with_alpha(
            egui::Color32::from_rgb(232, 238, 246),
            scaled(u8::MAX, fade),
        ),
    );

    // Health bar — same fill colour ladder as `peer_overlay::health_fill_color`
    // so the visual language for "low HP" matches across player and
    // structure labels.
    let max = state.max_health.max(1) as f32;
    let fraction = ((state.health as f32) / max).clamp(0.0, 1.0);
    let bar_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 8.0, rect.bottom() - 9.0),
        egui::pos2(rect.right() - 8.0, rect.bottom() - 9.0 + HEALTH_BAR_HEIGHT),
    );
    let fill_rect = egui::Rect::from_min_max(
        bar_rect.min,
        egui::pos2(
            bar_rect.left() + bar_rect.width() * fraction,
            bar_rect.bottom(),
        ),
    );
    ui.painter().rect_filled(
        bar_rect,
        1,
        egui::Color32::from_rgba_unmultiplied(30, 32, 38, scaled(220, fade)),
    );
    ui.painter().rect_filled(
        fill_rect,
        1,
        with_alpha(health_fill_color(fraction), scaled(u8::MAX, fade)),
    );
}

fn kind_label(kind: DeployableKind) -> String {
    match kind {
        DeployableKind::Workbench { tier } => format!("Workbench T{tier}"),
        DeployableKind::Furnace { tier } => format!("Furnace T{tier}"),
    }
}

fn distance_fade(distance: f32) -> f32 {
    if distance <= DEPLOYABLE_FADE_START_M {
        return 1.0;
    }
    let span = (DEPLOYABLE_DRAW_DISTANCE_M - DEPLOYABLE_FADE_START_M).max(0.001);
    let into = (distance - DEPLOYABLE_FADE_START_M).clamp(0.0, span);
    1.0 - into / span
}

fn scaled(base: u8, fade: f32) -> u8 {
    (f32::from(base) * fade.clamp(0.0, 1.0)).round() as u8
}

fn with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
    let [r, g, b, _] = color.to_array();
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

fn health_fill_color(fraction: f32) -> egui::Color32 {
    if fraction > 0.6 {
        egui::Color32::from_rgb(125, 196, 55)
    } else if fraction > 0.3 {
        egui::Color32::from_rgb(232, 188, 64)
    } else {
        egui::Color32::from_rgb(228, 96, 78)
    }
}

/// Pair each placed-entity transform with its current `DeployedEntityState`
/// from the snapshot. The anchor world point is the structure centre +
/// `NAMEPLATE_TOP_CLEARANCE_M`, which sits above the tallest current
/// deployable (the furnace chimney) without depending on per-kind mesh
/// heights.
pub(crate) fn collect_deployable_overlay_entries<'a>(
    placed_entities: impl IntoIterator<Item = (&'a NetworkDeployedEntity, &'a GlobalTransform)>,
    snapshot_entities: impl IntoIterator<Item = &'a DeployedEntityState>,
) -> Vec<DeployableOverlayEntry<'a>> {
    let mut state_by_id: std::collections::HashMap<u64, &DeployedEntityState> = snapshot_entities
        .into_iter()
        .map(|state| (state.id, state))
        .collect();

    placed_entities
        .into_iter()
        .filter_map(|(entity, transform)| {
            let state = state_by_id.remove(&entity.id)?;
            let translation = transform.translation();
            let anchor_world = translation + Vec3::Y * NAMEPLATE_TOP_CLEARANCE_M;
            Some(DeployableOverlayEntry {
                anchor_world,
                state,
            })
        })
        .collect()
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DeployableOverlayParams<'w, 's> {
    pub(crate) placed: Query<'w, 's, (&'static NetworkDeployedEntity, &'static GlobalTransform)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_fade_starts_full_then_drops_to_zero() {
        assert!((distance_fade(0.0) - 1.0).abs() < f32::EPSILON);
        assert!((distance_fade(DEPLOYABLE_FADE_START_M) - 1.0).abs() < f32::EPSILON);
        assert!(distance_fade(DEPLOYABLE_DRAW_DISTANCE_M + 1.0) <= 0.0001);
    }

    #[test]
    fn kind_label_includes_tier() {
        assert_eq!(
            kind_label(DeployableKind::Workbench { tier: 1 }),
            "Workbench T1"
        );
        assert_eq!(
            kind_label(DeployableKind::Furnace { tier: 2 }),
            "Furnace T2"
        );
    }
}
