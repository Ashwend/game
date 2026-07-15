//! Combat feedback overlays: the on-crosshair hit marker and the damage
//! direction chevrons ringing it. Both are screen-space painters on dedicated
//! full-screen layers, driven by `CombatFeedbackState`.

use bevy_egui::egui;

use crate::app::state::CombatFeedbackState;

/// On-crosshair hit marker plus the damage-direction arrows. Both are
/// screen-space overlays painted on dedicated full-screen layers so they read
/// against the world without an Area clipping them. `px`/`pz` are the local
/// player's horizontal position and `yaw` their look heading, used to place the
/// arrows relative to where the player is facing.
pub(super) fn combat_feedback_ui(
    ctx: &egui::Context,
    combat: &CombatFeedbackState,
    px: f32,
    pz: f32,
    yaw: f32,
) {
    hit_marker(ctx, combat.hit_marker_fade(), combat.hit_marker_is_player());
    damage_direction(ctx, combat, px, pz, yaw);

    // Animate the fades while anything is on-screen.
    if combat.is_active() {
        ctx.request_repaint();
    }
}

/// Classic four-tick "X" hit marker at screen center that fades and expands
/// slightly as it decays. White for world hits, hot red for PvP connects.
fn hit_marker(ctx: &egui::Context, fade: f32, is_player: bool) {
    if fade <= 0.0 {
        return;
    }
    let center = ctx.content_rect().center();
    // Grow the gap as it fades so the marker reads as a quick "pop".
    let grow = 1.0 + (1.0 - fade) * 0.6;
    let gap = 4.0 * grow;
    let len = 7.0;
    let alpha = (fade * 235.0) as u8;
    let color = if is_player {
        egui::Color32::from_rgba_unmultiplied(255, 90, 78, alpha)
    } else {
        egui::Color32::from_rgba_unmultiplied(240, 242, 245, alpha)
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("hit_marker"),
    ));
    let stroke = egui::Stroke::new(2.0, color);
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let inner = egui::pos2(center.x + sx * gap, center.y + sy * gap);
        let outer = egui::pos2(center.x + sx * (gap + len), center.y + sy * (gap + len));
        painter.line_segment([inner, outer], stroke);
    }
}

/// Red chevrons on a ring around the crosshair, one per recent hit, each
/// pointing toward the attacker's bearing relative to the player's facing.
/// Fades with each arrow's remaining lifetime.
fn damage_direction(ctx: &egui::Context, combat: &CombatFeedbackState, px: f32, pz: f32, yaw: f32) {
    let arrows = combat.damage_arrows();
    if arrows.is_empty() {
        return;
    }
    let rect = ctx.content_rect();
    let center = rect.center();
    let radius = rect.size().min_elem() * 0.18;

    // Horizontal forward + right basis from the look heading. `look_forward`
    // owns the yaw convention; `right = forward x up` lines up with egui's
    // screen axes (x right, y down) so a hit from world-right shows on the
    // right of the ring.
    let forward = crate::items::look_forward(yaw, 0.0);
    let flen = (forward.x * forward.x + forward.z * forward.z)
        .sqrt()
        .max(1e-4);
    let (fx, fz) = (forward.x / flen, forward.z / flen);

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("damage_direction"),
    ));
    for arrow in arrows {
        let dx = arrow.source.x - px;
        let dz = arrow.source.z - pz;
        if dx * dx + dz * dz < 1e-4 {
            continue;
        }
        let ahead = dx * fx + dz * fz;
        let side = dx * (-fz) + dz * fx;
        // angle 0 = directly ahead -> top of the ring; +angle (world-right) ->
        // right side of the ring.
        let angle = side.atan2(ahead);
        let dir = egui::vec2(angle.sin(), -angle.cos());
        let tangent = egui::vec2(-dir.y, dir.x);
        let pos = center + dir * radius;
        let tip = pos + dir * 13.0;
        let base_a = pos - dir * 4.0 + tangent * 9.0;
        let base_b = pos - dir * 4.0 - tangent * 9.0;
        let alpha = (arrow.fade() * 220.0) as u8;
        let fill = egui::Color32::from_rgba_unmultiplied(214, 40, 40, alpha);
        painter.add(egui::Shape::convex_polygon(
            vec![tip, base_a, base_b],
            fill,
            egui::Stroke::NONE,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::super::raw_input;
    use super::*;

    #[test]
    fn combat_feedback_overlay_draws_when_active() {
        // Idle state: no hit marker, no arrows, nothing painted.
        let ctx = egui::Context::default();
        let idle = ctx.run_ui(raw_input(), |ui| {
            combat_feedback_ui(ui.ctx(), &CombatFeedbackState::default(), 0.0, 0.0, 0.0);
        });
        assert!(idle.shapes.is_empty());

        // Active marker + a directional arrow both emit shapes.
        let mut combat = CombatFeedbackState::default();
        combat.trigger_hit_marker(true);
        combat.push_damage_from(bevy::math::Vec3::new(5.0, 0.0, 5.0));
        let ctx = egui::Context::default();
        let active = ctx.run_ui(raw_input(), |ui| {
            combat_feedback_ui(ui.ctx(), &combat, 0.0, 0.0, 0.0);
        });
        assert!(!active.shapes.is_empty());
    }
}
