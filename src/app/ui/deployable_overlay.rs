//! Floating nameplate + health bar above damaged placed structures.
//!
//! Spec from the design pass: only show the label when the player
//!  - is **looking at** the structure (camera-forward cone test),
//!  - is **near** it (≤ 6 m from the structure's centre),
//!  - the structure is **not at full health**.
//!
//! Falls back to silence in every other case so undamaged props don't
//! clutter the screen. Same screen-projection / fade-out / viewport-
//! clamp pattern as [`super::peer_overlay`], extracted helpers stay
//! local for now since deployable labels are simpler (no chat bubble,
//! no voice indicator).

use bevy::prelude::*;
use bevy_egui::egui;

use crate::{
    app::scene::NetworkDeployedEntity,
    game_balance::DEPLOYABLE_DAMAGE_RANGE_M,
    items::{DeployableKind, item_definition},
    server::{Deployable, DeployableHealth},
};

/// Distance at which the nameplate is fully solid: the reach at which you can
/// actually hit the structure, so the health bar is readable wherever a swing
/// lands. Derived from [`DEPLOYABLE_DAMAGE_RANGE_M`] so the label tracks the
/// real interact range instead of drifting on its own magic number.
const DEPLOYABLE_FADE_START_M: f32 = DEPLOYABLE_DAMAGE_RANGE_M;
/// Hard cutoff distance for the nameplate. One metre past the reach limit, so
/// the label dissolves softly just as you step out of range.
const DEPLOYABLE_DRAW_DISTANCE_M: f32 = DEPLOYABLE_DAMAGE_RANGE_M + 1.0;
/// Cosine of the half-angle of the "looking at" cone. ~25° each side
/// of the camera forward, generous enough that you don't have to
/// pixel-aim, tight enough that you can't see labels on structures
/// off to your peripheral vision.
const DEPLOYABLE_LOOK_CONE_COS: f32 = 0.91;
/// Inset (logical pixels) before the viewport edge.
const DEPLOYABLE_VIEWPORT_INSET_PX: f32 = 12.0;
/// Vertical gap (world space) between the top of the structure's
/// collider and the nameplate baseline. Small constant, actual
/// nameplate height comes from the structure's own collider so a
/// short workbench gets a low label and a taller furnace gets a
/// higher one.
const NAMEPLATE_TOP_CLEARANCE_M: f32 = 0.2;

const NAMETAG_WIDTH: f32 = 144.0;
const NAMETAG_HEIGHT: f32 = 32.0;
const HEALTH_BAR_HEIGHT: f32 = 4.0;

pub(crate) struct DeployableOverlay<'world> {
    pub(crate) camera: Option<(&'world Camera, GlobalTransform)>,
    pub(crate) entries: Vec<DeployableOverlayEntry>,
}

pub(crate) struct DeployableOverlayEntry {
    /// Where the nameplate sits in world space, just above the
    /// structure's top.
    pub(crate) anchor_world: Vec3,
    /// Point used for the look-cone + range checks. The structure's
    /// vertical centre rather than the floating nameplate anchor, so
    /// looking at the *body* of the workbench (not above it) still
    /// counts as "aiming at it."
    pub(crate) look_target_world: Vec3,
    pub(crate) id: u64,
    pub(crate) kind: DeployableKind,
    pub(crate) health: u32,
    pub(crate) max_health: u32,
}

pub(super) fn deployable_overlay_ui(ctx: &egui::Context, overlay: DeployableOverlay<'_>) {
    let Some((camera, camera_transform)) = overlay.camera else {
        return;
    };
    let camera_forward = camera_transform.forward().as_vec3();
    let camera_origin = camera_transform.translation();
    let visible_rect = ctx.content_rect().shrink(DEPLOYABLE_VIEWPORT_INSET_PX);

    for entry in overlay.entries {
        // Range + cone test use the structure's centre (`look_target_world`),
        // not the floating label anchor, otherwise looking at the
        // workbench body misses the cone and the label stays hidden
        // until the player tilts the camera up at the empty space
        // above the structure.
        let to_target = entry.look_target_world - camera_origin;
        let distance = to_target.length();
        if distance > DEPLOYABLE_DRAW_DISTANCE_M {
            continue;
        }
        // Looking-at test: dot the unit ray to the structure with
        // camera forward. Below the cone cosine → not centred enough
        // to be considered "the player is looking at it."
        let cosine = if distance > 1e-3 {
            to_target.dot(camera_forward) / distance
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
        // Always allocate the Area while the player is aiming at the
        // structure, even at full health. egui's first-show sizing
        // pass renders invisibly for one frame and fades in over the
        // next few, if the Area only appeared the instant the
        // structure took its first hit, the nameplate would stay
        // hidden until the player swung the camera off and back. By
        // pre-warming here, the sizing pass + fade-in happen once
        // when the structure first comes into focus.
        let damaged = entry.health < entry.max_health;
        draw_deployable_label(ctx, screen, distance, &entry, damaged);
    }
}

fn draw_deployable_label(
    ctx: &egui::Context,
    screen: Vec2,
    distance: f32,
    entry: &DeployableOverlayEntry,
    damaged: bool,
) {
    let id = egui::Id::new(("deployable_overlay", entry.id));
    // Multiply the distance fade by 0 when the structure is at full
    // health: the Area still renders (keeping its size + visibility
    // memo warm in egui) but is fully transparent, so undamaged props
    // stay silent.
    let fade = if damaged {
        distance_fade(distance)
    } else {
        0.0
    };

    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .interactable(false)
        .movable(false)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(screen.x, screen.y))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                nameplate(ui, entry, fade);
            });
        });
}

fn nameplate(ui: &mut egui::Ui, entry: &DeployableOverlayEntry, fade: f32) {
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

    let label = kind_label(entry.kind);
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

    // Health bar, same fill colour ladder as `peer_overlay::health_fill_color`
    // so the visual language for "low HP" matches across player and
    // structure labels.
    let max = entry.max_health.max(1) as f32;
    let fraction = ((entry.health as f32) / max).clamp(0.0, 1.0);
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

/// Pair each placed-entity visual transform with the matching
/// replicated `(Deployable, DeployableHealth)` for the nameplate.
/// Heights come from the structure's own collider profile so the
/// nameplate sits just above the visible top (workbench vs furnace
/// differ in height) and the look-cone target sits at the vertical
/// centre of the body.
pub(crate) fn collect_deployable_overlay_entries<'a>(
    placed_entities: impl IntoIterator<Item = (&'a NetworkDeployedEntity, &'a GlobalTransform)>,
    replicated: impl IntoIterator<Item = (&'a Deployable, &'a DeployableHealth)>,
) -> Vec<DeployableOverlayEntry> {
    let mut state_by_id: std::collections::HashMap<u64, (&Deployable, &DeployableHealth)> =
        replicated
            .into_iter()
            .map(|(meta, health)| (meta.id, (meta, health)))
            .collect();

    placed_entities
        .into_iter()
        .filter_map(|(entity, transform)| {
            let (meta, health) = state_by_id.remove(&entity.id)?;
            let profile = item_definition(&meta.item_id).and_then(|def| def.deployable)?;
            let translation = transform.translation();
            let full_height = profile.collider_half_height * 2.0;
            let look_target_world = translation + Vec3::Y * profile.collider_half_height;
            let anchor_world = translation + Vec3::Y * (full_height + NAMEPLATE_TOP_CLEARANCE_M);
            Some(DeployableOverlayEntry {
                anchor_world,
                look_target_world,
                id: meta.id,
                kind: meta.kind,
                health: health.0,
                max_health: meta.max_health,
            })
        })
        .collect()
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DeployableOverlayParams<'w, 's> {
    pub(crate) placed: Query<'w, 's, (&'static NetworkDeployedEntity, &'static GlobalTransform)>,
    pub(crate) replicated: Query<'w, 's, (&'static Deployable, &'static DeployableHealth)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{CRUDE_FURNACE_ID, WORKBENCH_T1_ID, intern_item_id};

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 768.0),
            )),
            ..Default::default()
        }
    }

    fn entry(health: u32, max_health: u32) -> DeployableOverlayEntry {
        DeployableOverlayEntry {
            anchor_world: Vec3::Y * 2.0,
            look_target_world: Vec3::Y,
            id: 7,
            kind: DeployableKind::Workbench { tier: 1 },
            health,
            max_health,
        }
    }

    #[test]
    fn distance_fade_starts_full_then_drops_to_zero() {
        assert!((distance_fade(0.0) - 1.0).abs() < f32::EPSILON);
        assert!((distance_fade(DEPLOYABLE_FADE_START_M) - 1.0).abs() < f32::EPSILON);
        assert!(distance_fade(DEPLOYABLE_DRAW_DISTANCE_M + 1.0) <= 0.0001);
        // Midway through the fade band the alpha is partial.
        let mid = distance_fade((DEPLOYABLE_FADE_START_M + DEPLOYABLE_DRAW_DISTANCE_M) / 2.0);
        assert!(mid > 0.0 && mid < 1.0);
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

    #[test]
    fn health_fill_color_maps_three_tiers() {
        assert_eq!(
            health_fill_color(1.0),
            egui::Color32::from_rgb(125, 196, 55)
        );
        assert_eq!(
            health_fill_color(0.5),
            egui::Color32::from_rgb(232, 188, 64)
        );
        assert_eq!(health_fill_color(0.1), egui::Color32::from_rgb(228, 96, 78));
        // Strict `>` boundaries fall to the lower band.
        assert_eq!(
            health_fill_color(0.6),
            egui::Color32::from_rgb(232, 188, 64)
        );
        assert_eq!(health_fill_color(0.3), egui::Color32::from_rgb(228, 96, 78));
    }

    #[test]
    fn scaled_and_with_alpha_apply_and_clamp_fade() {
        assert_eq!(scaled(100, 1.0), 100);
        assert_eq!(scaled(100, 0.0), 0);
        assert_eq!(scaled(100, 0.5), 50);
        assert_eq!(scaled(100, 5.0), 100);
        assert_eq!(scaled(100, -2.0), 0);
        assert_eq!(
            with_alpha(egui::Color32::from_rgb(9, 8, 7), 200),
            egui::Color32::from_rgba_unmultiplied(9, 8, 7, 200)
        );
        assert_eq!(
            with_alpha(egui::Color32::from_rgb(9, 8, 7), 255),
            egui::Color32::from_rgb(9, 8, 7)
        );
    }

    #[test]
    fn overlay_without_camera_draws_nothing() {
        let ctx = egui::Context::default();
        let output = ctx.run(raw_input(), |ctx| {
            deployable_overlay_ui(
                ctx,
                DeployableOverlay {
                    camera: None,
                    entries: Vec::new(),
                },
            );
        });
        assert!(output.shapes.is_empty());
    }

    #[test]
    fn nameplate_renders_at_full_and_damaged_fade() {
        // Full-health props draw with fade 0 (transparent) but still emit
        // shapes; damaged ones draw fully opaque. Both run the painter.
        let full = entry(500, 500);
        let damaged = entry(120, 500);

        let ctx_full = egui::Context::default();
        let out_full = ctx_full.run(raw_input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                nameplate(ui, &full, 0.0);
            });
        });
        let ctx_dmg = egui::Context::default();
        let out_dmg = ctx_dmg.run(raw_input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                nameplate(ui, &damaged, 1.0);
            });
        });
        assert!(!out_full.shapes.is_empty());
        assert!(!out_dmg.shapes.is_empty());
    }

    #[test]
    fn collect_entries_pairs_placed_with_replicated_state() {
        let placed_wb = NetworkDeployedEntity {
            id: 10,
            kind: DeployableKind::Workbench { tier: 1 },
        };
        let placed_furnace = NetworkDeployedEntity {
            id: 11,
            kind: DeployableKind::Furnace { tier: 1 },
        };
        // No replicated state for this id: it should be filtered out.
        let placed_orphan = NetworkDeployedEntity {
            id: 99,
            kind: DeployableKind::Workbench { tier: 1 },
        };

        let tf = GlobalTransform::from_translation(Vec3::new(2.0, 0.0, -3.0));

        let dep_wb = Deployable {
            id: 10,
            item_id: intern_item_id(WORKBENCH_T1_ID),
            kind: DeployableKind::Workbench { tier: 1 },
            max_health: 500,
        };
        let dep_furnace = Deployable {
            id: 11,
            item_id: intern_item_id(CRUDE_FURNACE_ID),
            kind: DeployableKind::Furnace { tier: 1 },
            max_health: 800,
        };
        let hp_wb = DeployableHealth(250);
        let hp_furnace = DeployableHealth(800);

        let placed = vec![
            (&placed_wb, &tf),
            (&placed_furnace, &tf),
            (&placed_orphan, &tf),
        ];
        let replicated = vec![(&dep_wb, &hp_wb), (&dep_furnace, &hp_furnace)];

        let mut entries = collect_deployable_overlay_entries(placed, replicated);
        entries.sort_by_key(|e| e.id);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, 10);
        assert_eq!(entries[0].health, 250);
        assert_eq!(entries[0].max_health, 500);
        assert_eq!(entries[1].id, 11);
        assert_eq!(entries[1].health, 800);
        // Furnace is taller, so its nameplate anchor sits higher than the
        // workbench's for the same ground transform.
        assert!(entries[1].anchor_world.y > entries[0].anchor_world.y);
        // Look target is below the anchor (body centre, not floating label).
        assert!(entries[0].look_target_world.y < entries[0].anchor_world.y);
    }
}
