//! Meteor shower HUD: the escalating danger-zone evacuation warning, shown
//! only while the local player stands inside a live meteor's size-scaled
//! danger radius. There is deliberately NO global announcement UI (no
//! countdown pill, no map marker): the sky fireballs and the audio are the
//! announcement, and this warning is the one piece of chrome, computed
//! client-side from the announce payload against the authoritative clock
//! estimate so it costs nothing on the wire.

use bevy_egui::egui;

use crate::app::state::ClientRuntime;

/// Escalation intensity `0.0..=1.0` for the danger-zone evacuation warning,
/// ramping over the final 60 seconds before impact. `0.0` at 60 s out, `1.0` at
/// impact, held at `1.0` briefly after. Pure so the threshold is unit-testable.
fn meteor_shower_danger_intensity(seconds_to_impact: f32) -> f32 {
    // Before the last minute the warning is present but calm (a small floor);
    // in the final 60 s it climbs to full.
    ((60.0 - seconds_to_impact) / 60.0).clamp(0.0, 1.0)
}

/// The danger-zone evacuation warning: shown only while the player's OWN
/// position is inside the size-scaled danger radius of a live, not-yet-landed
/// meteor. Evaluated against EVERY meteor of the shower; the most imminent
/// covering meteor drives the escalation. Computed client-side from the
/// announce payload (`runtime.meteor_showers`) plus the player's own position
/// against the authoritative clock estimate. Silent when no meteor threatens
/// the player's spot.
pub(super) fn meteor_shower_hud(ctx: &egui::Context, runtime: &ClientRuntime) {
    if runtime.meteor_showers.is_empty() {
        return;
    }
    let Some(player) = runtime.local_view() else {
        return;
    };
    let now = runtime.server_tick();

    // The most imminent meteor whose (size-scaled) danger radius covers the
    // player. Because impact siting guarantees the clearance exceeds the
    // blast, the warning directs players OUT of the zone rather than under a
    // roof that cannot exist inside it.
    let mut intensity: Option<f32> = None;
    for event in &runtime.meteor_showers {
        // Only the pre-impact phase warns; after a meteor lands its crater
        // visual takes over.
        if event.has_impacted(now) {
            continue;
        }
        let danger_radius = crate::game_balance::METEOR_SHOWER_DANGER_RADIUS_M * event.size;
        if !event
            .impact_position
            .within_horizontal_range(player.position, danger_radius)
        {
            continue;
        }
        let candidate = meteor_shower_danger_intensity(event.seconds_to_impact(now));
        intensity = Some(intensity.map_or(candidate, |best: f32| best.max(candidate)));
    }
    if let Some(intensity) = intensity {
        meteor_shower_danger_warning(ctx, intensity);
    }
}

/// The escalating "evacuate the area" banner shown while the player stands inside
/// a meteor's danger zone. Its colour saturates and it pulses faster as
/// `intensity` (0..=1, ramping over the final 60 s) climbs.
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
                            egui::RichText::new("Meteor incoming. Evacuate the area.")
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
