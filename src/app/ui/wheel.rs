//! Radial wheel overlay rendering. Pure painting: input (open / pointer /
//! commit) lives in `app::systems::input::wheel`; this draws the sectors,
//! labels, and selection highlight for whatever `WheelMenuState` holds.

use bevy_egui::egui;

use crate::app::state::{ActiveWheel, WHEEL_DEADZONE_PX, WheelMenuState};

const WHEEL_RADIUS: f32 = 150.0;
const WHEEL_INNER_RADIUS: f32 = 44.0;
const LABEL_RADIUS: f32 = 102.0;

pub(super) fn wheel_ui(ctx: &egui::Context, wheel: &WheelMenuState) {
    let Some(active) = wheel.active.as_ref() else {
        return;
    };

    egui::Area::new(egui::Id::new("radial_wheel"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(0.0, 0.0))
        .show(ctx, |ui| {
            let center = ctx.content_rect().center();
            let painter = ui.painter();
            draw_wheel(painter, center, active);
        });
}

fn draw_wheel(painter: &egui::Painter, center: egui::Pos2, active: &ActiveWheel) {
    let selected = active.selected_index();
    let count = active.options.len().max(1);
    let span = std::f32::consts::TAU / count as f32;

    // Backdrop disc.
    painter.circle_filled(
        center,
        WHEEL_RADIUS + 8.0,
        egui::Color32::from_rgba_unmultiplied(12, 14, 16, 188),
    );

    for (index, option) in active.options.iter().enumerate() {
        // Sector 0 centred at 12 o'clock, clockwise.
        let mid = index as f32 * span - std::f32::consts::FRAC_PI_2;
        let is_selected = selected == Some(index);

        // Sector wedge as a filled fan of small triangles (egui has no
        // arc primitive); subtle for idle sectors, bright for the pick.
        let fill = if is_selected && option.enabled {
            egui::Color32::from_rgba_unmultiplied(120, 156, 110, 110)
        } else if is_selected {
            egui::Color32::from_rgba_unmultiplied(150, 84, 80, 90)
        } else {
            egui::Color32::from_rgba_unmultiplied(56, 60, 64, 60)
        };
        let steps = 10;
        let a0 = mid - span / 2.0 + 0.015;
        let a1 = mid + span / 2.0 - 0.015;
        for step in 0..steps {
            let t0 = a0 + (a1 - a0) * (step as f32 / steps as f32);
            let t1 = a0 + (a1 - a0) * ((step + 1) as f32 / steps as f32);
            let points = vec![
                center + egui::vec2(t0.cos(), t0.sin()) * WHEEL_INNER_RADIUS,
                center + egui::vec2(t0.cos(), t0.sin()) * WHEEL_RADIUS,
                center + egui::vec2(t1.cos(), t1.sin()) * WHEEL_RADIUS,
                center + egui::vec2(t1.cos(), t1.sin()) * WHEEL_INNER_RADIUS,
            ];
            painter.add(egui::Shape::convex_polygon(
                points,
                fill,
                egui::Stroke::NONE,
            ));
        }

        let label_pos = center + egui::vec2(mid.cos(), mid.sin()) * LABEL_RADIUS;
        let text_color = if !option.enabled {
            egui::Color32::from_gray(110)
        } else if is_selected {
            egui::Color32::from_rgb(235, 240, 228)
        } else {
            egui::Color32::from_gray(200)
        };
        let label_rect = painter.text(
            label_pos,
            egui::Align2::CENTER_CENTER,
            &option.label,
            egui::FontId::proportional(15.0),
            text_color,
        );
        // Marker dot for the currently-selected plan piece. Painted, not
        // a bullet character: the UI font has no glyph for those and
        // renders a tofu rectangle instead.
        if option.marked {
            painter.circle_filled(
                egui::pos2(label_rect.left() - 9.0, label_rect.center().y),
                3.0,
                egui::Color32::from_rgb(214, 178, 96),
            );
        }
        if let Some(detail) = &option.detail {
            // The eligibility readout: red when the requirement (cost,
            // ownership) isn't currently met. The option stays selectable,
            // the server's toast explains the refusal.
            let detail_color = if option.detail_ok {
                egui::Color32::from_gray(150)
            } else {
                egui::Color32::from_rgb(216, 96, 88)
            };
            painter.text(
                label_pos + egui::vec2(0.0, 16.0),
                egui::Align2::CENTER_CENTER,
                detail,
                egui::FontId::proportional(11.0),
                detail_color,
            );
        }
    }

    // Centre hub: wheel title + a small pointer nub showing the
    // accumulated drag direction.
    painter.circle_filled(
        center,
        WHEEL_INNER_RADIUS - 6.0,
        egui::Color32::from_rgba_unmultiplied(20, 22, 25, 220),
    );
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        &active.title,
        egui::FontId::proportional(13.0),
        egui::Color32::from_gray(220),
    );
    if active.pointer.length() > WHEEL_DEADZONE_PX {
        let direction = active.pointer.normalize_or_zero();
        let nub = center + egui::vec2(direction.x, direction.y) * (WHEEL_INNER_RADIUS - 12.0);
        painter.circle_filled(nub, 3.0, egui::Color32::from_gray(235));
    }
}
