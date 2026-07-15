//! Meteor shower HUD: the CENTER_TOP countdown pill plus the escalating
//! danger-zone evacuation warning. Both are computed client-side from the
//! announce payload against the authoritative clock estimate, so they cost
//! nothing on the wire and can never desync from the countdown.

use bevy_egui::egui;

use crate::app::state::ClientRuntime;

use super::super::theme;

/// Format a meteor shower countdown for the HUD pill from the seconds remaining to
/// impact. Under 30 s it switches to a terse "Impact imminent"; otherwise it
/// reads `M:SS`. Pure so it can be unit-tested.
fn format_meteor_shower_countdown(seconds_to_impact: f32) -> String {
    if seconds_to_impact <= 30.0 {
        return "Impact imminent".to_owned();
    }
    let total = seconds_to_impact.max(0.0) as u32;
    let minutes = total / 60;
    let secs = total % 60;
    format!("MeteorShower {minutes}:{secs:02}")
}

/// Escalation intensity `0.0..=1.0` for the danger-zone evacuation warning,
/// ramping over the final 60 seconds before impact. `0.0` at 60 s out, `1.0` at
/// impact, held at `1.0` briefly after. Pure so the threshold is unit-testable.
fn meteor_shower_danger_intensity(seconds_to_impact: f32) -> f32 {
    // Before the last minute the warning is present but calm (a small floor);
    // in the final 60 s it climbs to full.
    ((60.0 - seconds_to_impact) / 60.0).clamp(0.0, 1.0)
}

/// The meteor shower countdown pill and the danger-zone evacuation warning. Both are
/// computed client-side from the announce payload (`runtime.meteor_shower`) plus the
/// player's own position against the authoritative clock estimate, so they cost
/// nothing on the wire and can never desync from the countdown. Silent when no
/// event is live or after impact.
pub(super) fn meteor_shower_hud(ctx: &egui::Context, runtime: &ClientRuntime) {
    let Some(event) = runtime.meteor_shower else {
        return;
    };
    let now = runtime.server_tick();
    // Only the pre-impact fireball phase gets a countdown; after impact the pill
    // and warning go quiet (the crater visual takes over).
    if event.has_impacted(now) {
        return;
    }
    let seconds_to_impact = event.seconds_to_impact(now);

    // Countdown pill: CENTER_TOP, the voice-indicator template.
    let imminent = seconds_to_impact <= 30.0;
    let (fill, stroke, text_color) = if imminent {
        (
            egui::Color32::from_rgba_unmultiplied(38, 10, 6, 235),
            egui::Color32::from_rgba_unmultiplied(255, 120, 70, 200),
            egui::Color32::from_rgb(255, 190, 150),
        )
    } else {
        (
            egui::Color32::from_rgba_unmultiplied(10, 8, 12, 220),
            egui::Color32::from_rgba_unmultiplied(248, 150, 70, 110),
            theme::text(),
        )
    };
    let label = format_meteor_shower_countdown(seconds_to_impact);
    egui::Area::new("meteor_shower_countdown".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 24.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, stroke))
                .corner_radius(12)
                .inner_margin(egui::Margin::symmetric(14, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // A small painter-drawn ember dot, warmer/pulsing when
                        // imminent (no font glyph dependency, same reason as the
                        // voice indicator).
                        let dot_radius = 4.5;
                        let (dot_rect, _) = ui.allocate_exact_size(
                            egui::vec2(dot_radius * 2.0, dot_radius * 2.0),
                            egui::Sense::hover(),
                        );
                        let pulse = if imminent {
                            let t = ctx.input(|input| input.time) as f32;
                            0.5 + 0.5 * (t * std::f32::consts::TAU * 1.6).sin()
                        } else {
                            1.0
                        };
                        let dot_alpha = (150.0 + pulse * 105.0) as u8;
                        ui.painter().circle_filled(
                            dot_rect.center(),
                            dot_radius,
                            egui::Color32::from_rgba_unmultiplied(255, 120, 60, dot_alpha),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new(label)
                                .size(13.0)
                                .strong()
                                .color(text_color),
                        );
                    });
                });
        });
    if imminent {
        ctx.request_repaint();
    }

    // Danger-zone evacuation warning: only when the player's OWN position is
    // inside the danger radius of the impact point. Escalates over the final 60 s
    // (colour + pulse). Because the impact siting guarantees the clearance
    // exceeds the blast, the warning directs players OUT of the zone rather than
    // under a roof that cannot exist inside it.
    let Some(player) = runtime.local_view() else {
        return;
    };
    let inside_danger = event.impact_position.within_horizontal_range(
        player.position,
        crate::game_balance::METEOR_SHOWER_DANGER_RADIUS_M,
    );
    if !inside_danger {
        return;
    }
    let intensity = meteor_shower_danger_intensity(seconds_to_impact);
    meteor_shower_danger_warning(ctx, intensity);
}

/// The escalating "evacuate the area" banner shown while the player stands inside
/// the danger zone. Sits below the countdown pill; its colour saturates and it
/// pulses faster as `intensity` (0..=1, ramping over the final 60 s) climbs.
fn meteor_shower_danger_warning(ctx: &egui::Context, intensity: f32) {
    let intensity = intensity.clamp(0.0, 1.0);
    let t = ctx.input(|input| input.time) as f32;
    // Pulse frequency rises with intensity so it feels more urgent late.
    let freq = 1.0 + intensity * 3.0;
    let pulse = 0.5 + 0.5 * (t * std::f32::consts::TAU * freq).sin();
    // Border/background redden and brighten with intensity + pulse.
    let base = 60.0 + intensity * 140.0;
    let glow = (base + pulse * 55.0).min(255.0) as u8;
    let fill_alpha = (150.0 + intensity * 90.0) as u8;

    egui::Area::new("meteor_shower_danger".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 64.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(48, 6, 4, fill_alpha))
                .stroke(egui::Stroke::new(
                    1.5 + intensity * 1.5,
                    egui::Color32::from_rgba_unmultiplied(glow, 40, 24, 235),
                ))
                .corner_radius(10)
                .inner_margin(egui::Margin::symmetric(16, 8))
                .show(ui, |ui| {
                    let green = 200u8.saturating_sub((intensity * 80.0) as u8);
                    // Extend, never wrap: the banner must stay one line however
                    // large the late-intensity text grows.
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("Meteor shower incoming. Evacuate the area.")
                                .size(15.0 + intensity * 3.0)
                                .strong()
                                .color(egui::Color32::from_rgb(255, green, 170)),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
        });
    // Always animating while the warning is up.
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meteor_shower_countdown_formats_minutes_and_switches_to_imminent() {
        // Well out: M:SS with zero-padded seconds.
        assert_eq!(format_meteor_shower_countdown(504.0), "MeteorShower 8:24");
        assert_eq!(format_meteor_shower_countdown(65.0), "MeteorShower 1:05");
        // The 30 s boundary and below switches to the terse imminent copy.
        assert_eq!(format_meteor_shower_countdown(31.0), "MeteorShower 0:31");
        assert_eq!(format_meteor_shower_countdown(30.0), "Impact imminent");
        assert_eq!(format_meteor_shower_countdown(5.0), "Impact imminent");
        assert_eq!(format_meteor_shower_countdown(0.0), "Impact imminent");
    }

    #[test]
    fn meteor_shower_danger_intensity_ramps_over_the_final_minute() {
        // Before the last minute: no escalation yet.
        assert_eq!(meteor_shower_danger_intensity(120.0), 0.0);
        assert_eq!(meteor_shower_danger_intensity(60.0), 0.0);
        // Halfway through the final minute: ~half intensity.
        assert!((meteor_shower_danger_intensity(30.0) - 0.5).abs() < 1e-3);
        // At (and past) impact: full intensity, clamped.
        assert_eq!(meteor_shower_danger_intensity(0.0), 1.0);
        assert_eq!(meteor_shower_danger_intensity(-5.0), 1.0);
        // Monotonic as impact nears.
        assert!(meteor_shower_danger_intensity(10.0) > meteor_shower_danger_intensity(40.0));
    }
}
