//! Tool Cupboard claim standing indicator: the at-a-glance green/red bar
//! drawn directly above the health bar while the player stands inside some
//! cupboard's claimed footprint. Green = authorized on a covering cupboard
//! (build/upgrade/repair rights), red = covered but not authorized (someone
//! else's base). Outside every claim the caller draws nothing at all, so the
//! open world stays free of HUD chrome.
//!
//! Reads the owner-only replicated [`crate::server::PlayerClaimStatus`]
//! (assembled into `LocalPlayerState.private.claim_status`), so the verdict
//! is the server's, not a client-side guess.

use bevy_egui::egui;

// Same footprint width as the health bar below it (`health::HEALTH_WIDTH`),
// so the two read as one stacked cluster; deliberately slimmer, it is a
// status strip, not a gauge.
const CLAIM_WIDTH: f32 = 192.0;
const CLAIM_HEIGHT: f32 = 16.0;

pub(super) fn claim_indicator(ui: &mut egui::Ui, authorized: bool) {
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::new(CLAIM_WIDTH, CLAIM_HEIGHT),
        egui::Sense::hover(),
    );
    // Green tracks the health bar's fill; red is the HUD's danger tone.
    let (fill, label) = if authorized {
        (egui::Color32::from_rgb(125, 196, 55), "Authorized")
    } else {
        (egui::Color32::from_rgb(199, 62, 44), "Unauthorized")
    };
    // Dark backing panel like the health bar, with the status colour as a
    // translucent full-width fill so the bar itself is the signal and the
    // label is only confirmation.
    ui.painter().rect_filled(
        rect,
        1,
        egui::Color32::from_rgba_unmultiplied(30, 29, 24, 202),
    );
    ui.painter().rect_filled(
        rect.shrink(2.0),
        1,
        egui::Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 130),
    );
    ui.painter().rect_stroke(
        rect,
        1,
        egui::Stroke::new(1.0, fill),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(240, 247, 232),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_indicator_draws_both_standings() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(super::super::raw_input(), |ui| {
            claim_indicator(ui, true);
            claim_indicator(ui, false);
        });
        assert!(!output.shapes.is_empty(), "the indicator draws shapes");
        // Both labels present: the authorized and unauthorized strips render
        // their confirmation text.
        let text: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(text.iter().any(|t| t == "Authorized"));
        assert!(text.iter().any(|t| t == "Unauthorized"));
    }
}
