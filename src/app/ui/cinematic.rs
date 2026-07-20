//! The cinematic slate: countdown numbers, the preparing screen, and the
//! between-shots chip. Deliberately draws NOTHING while a shot is playing so
//! the captured frame is clean; the countdown and intermission surfaces exist
//! to be cut out in post, so they are informative rather than pretty.

use bevy_egui::egui;

use crate::app::state::{CinematicOverlay, CinematicOverlayPhase};
use crate::cinematic::script;

pub(super) fn cinematic_slate_ui(ctx: &egui::Context, overlay: &CinematicOverlay) {
    match overlay.phase {
        CinematicOverlayPhase::Playing { .. } => {}
        CinematicOverlayPhase::Preparing => preparing_ui(ctx, overlay),
        CinematicOverlayPhase::Countdown {
            shot_index,
            seconds,
        } => countdown_ui(ctx, overlay, shot_index, seconds),
        CinematicOverlayPhase::Intermission {
            next_shot_index, ..
        } => intermission_ui(ctx, next_shot_index),
    }
}

fn shot_title(shot_index: usize) -> String {
    match script::shot(shot_index) {
        Some(shot) => format!(
            "Shot {}/{}  \u{2022}  {}",
            shot_index + 1,
            script::SHOTS.len(),
            shot.name
        ),
        None => format!("Shot {}", shot_index + 1),
    }
}

fn dim(ctx: &egui::Context, alpha: u8) {
    egui::Area::new(egui::Id::new("cinematic_dim"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.painter().rect_filled(
                ctx.content_rect(),
                0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, alpha),
            );
        });
}

fn preparing_ui(ctx: &egui::Context, overlay: &CinematicOverlay) {
    dim(ctx, 200);
    egui::Area::new(egui::Id::new("cinematic_preparing"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("PREPARING STAGE")
                        .font(egui::FontId::new(34.0, egui::FontFamily::Proportional))
                        .color(egui::Color32::from_rgb(0xE8, 0xE2, 0xD4))
                        .strong(),
                );
                ui.add_space(10.0);
                // A slow dot pulse so the operator can tell the app is live.
                let dots = 1 + (overlay.elapsed as usize) % 3;
                ui.label(
                    egui::RichText::new(".".repeat(dots))
                        .font(egui::FontId::new(28.0, egui::FontFamily::Proportional))
                        .color(egui::Color32::from_rgb(0x9A, 0x94, 0x86)),
                );
            });
        });
    ctx.request_repaint();
}

fn countdown_ui(ctx: &egui::Context, overlay: &CinematicOverlay, shot_index: usize, seconds: f32) {
    // Light dim only: the operator should see the opening framing behind
    // the slate while OBS rolls.
    dim(ctx, 110);
    let remaining = (seconds - overlay.elapsed).max(0.0);
    let display = remaining.ceil().max(1.0) as u32;
    egui::Area::new(egui::Id::new("cinematic_countdown"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(shot_title(shot_index))
                        .font(egui::FontId::new(24.0, egui::FontFamily::Proportional))
                        .color(egui::Color32::from_rgb(0xC8, 0xC2, 0xB4)),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(display.to_string())
                        .font(egui::FontId::new(120.0, egui::FontFamily::Proportional))
                        .color(egui::Color32::from_rgb(0xF2, 0xEC, 0xDE))
                        .strong(),
                );
            });
        });
    ctx.request_repaint();
}

fn intermission_ui(ctx: &egui::Context, next_shot_index: Option<usize>) {
    // No dim at all: the held final frame stays clean under a small chip at
    // the top edge, easy to cut around.
    let text = match next_shot_index {
        Some(next) => format!("Holding \u{2022} next: {}", shot_title(next)),
        None => "Sequence complete".to_owned(),
    };
    egui::Area::new(egui::Id::new("cinematic_intermission"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 18.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 170))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .font(egui::FontId::new(18.0, egui::FontFamily::Proportional))
                            .color(egui::Color32::from_rgb(0xD8, 0xD2, 0xC4)),
                    );
                });
        });
    ctx.request_repaint();
}
