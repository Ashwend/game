//! "You died" splash + Respawn button.
//!
//! Server sends `ServerMessage::PlayerKilled` to the dying client; the
//! network tick stores a [`DeathSplash`] on `MenuState`. The splash
//! drives a two-stage UI:
//!
//! 1. A slow fade-to-black over `BLACK_FADE_SECS`. The player can
//!    still see the world for the first second or so — the camera
//!    stays pointed at wherever they died — and gradually loses
//!    contrast until the screen is fully black.
//! 2. After the black-out completes, the "YOU DIED" title, the
//!    "Killed by {name}" subline, and the Respawn button fade in.
//!
//! The respawn click sends `ClientMessage::Respawn`. The network
//! tick clears the splash when the server's `Correction` lands so
//! the camera comes back the moment the respawn settles, without a
//! second-long lag waiting on the replicated lifecycle component.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::{
    app::state::{ClientRuntime, DeathSplash, MenuState},
    protocol::ClientMessage,
};

/// Time, in seconds, from "the player died" to "screen is fully
/// black". A slow lift so the death moment lingers — the player
/// processes what happened before the UI takes over.
const BLACK_FADE_SECS: f32 = 4.0;
/// Time, in seconds, after the black-out completes before the
/// "YOU DIED" text + Respawn button finish their fade-in. Short
/// enough that the player isn't left staring at a black screen.
const TITLE_FADE_SECS: f32 = 0.6;
/// Time, in seconds, the splash spends fading back out once the
/// respawn lands. The window is short — the player wants their
/// view of the world back — but long enough that the HUD/hotbar
/// don't visibly pop into existence on a black screen.
const CLOSE_FADE_SECS: f32 = 0.45;

/// Render the death splash if present. Returns true when the player
/// just clicked Respawn so the caller can send the network message
/// from a system that owns the runtime mut-borrow.
pub(super) fn death_splash_ui(ctx: &egui::Context, splash: &DeathSplash) -> bool {
    let mut respawn_requested = false;

    // Two-phase alpha: rising while the player is dead, dropping
    // back to zero once the respawn lands. `multiplier` is 1.0 until
    // `begin_closing()` fires, then ramps down through the close-
    // fade window. Multiplying it onto the rise gives a smooth
    // "fade in, hold, fade out" without coupling the two timers.
    let rise = (splash.elapsed / BLACK_FADE_SECS).clamp(0.0, 1.0);
    let close_multiplier = splash
        .closing_elapsed
        .map(|t| 1.0 - (t / CLOSE_FADE_SECS).clamp(0.0, 1.0))
        .unwrap_or(1.0);
    let black = rise * close_multiplier;
    let backdrop_alpha = (black * 255.0).round() as u8;

    // Tooltip order sits above every world overlay (peer nametags,
    // floating damage text, deployable labels, hotbar HUD) so the
    // dim actually covers them. Background order put the dim
    // beneath everything else, which made the world fade out around
    // a still-visible HUD — exactly what the player flagged.
    egui::Area::new(egui::Id::new("death_splash_dim"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            let rect = ctx.content_rect();
            ui.painter().rect_filled(
                rect,
                0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, backdrop_alpha),
            );
        });

    // Title + button only meaningfully visible after the black fade is
    // most of the way done; clamp so the alpha never overshoots. Apply
    // the same close-fade multiplier so the title fades out alongside
    // the backdrop rather than blinking off when the splash clears.
    let title_rise = ((splash.elapsed - BLACK_FADE_SECS) / TITLE_FADE_SECS).clamp(0.0, 1.0);
    let title_alpha_f = title_rise * close_multiplier;
    let title_alpha = (title_alpha_f * 255.0).round() as u8;
    // While the title is still invisible, skip emitting the area
    // entirely — saves a layout pass and keeps the egui pointer
    // tracking from latching onto an invisible button.
    if title_alpha == 0 {
        return respawn_requested;
    }
    let red = egui::Color32::from_rgba_unmultiplied(0xCC, 0x33, 0x33, title_alpha);
    let subline_color = egui::Color32::from_rgba_unmultiplied(0xCC, 0xC8, 0xC0, title_alpha);

    egui::Area::new(egui::Id::new("death_splash"))
        // Title sits one tier above the dim so its drop-shadow text
        // and button can paint on top of the black backdrop without
        // any HUD widget poking through.
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("YOU DIED")
                        .font(egui::FontId::new(56.0, egui::FontFamily::Proportional))
                        .color(red)
                        .strong(),
                );
                ui.add_space(12.0);

                let subline = splash
                    .killer_name
                    .as_deref()
                    .map(|name| format!("Killed by {name}"))
                    .unwrap_or_else(|| "The world claimed you.".to_owned());
                ui.label(
                    egui::RichText::new(subline)
                        .font(egui::FontId::new(22.0, egui::FontFamily::Proportional))
                        .color(subline_color),
                );
                ui.add_space(28.0);

                // The respawn button uses the standard themed widget
                // so its visuals match the rest of the game UI; the
                // button itself doesn't fade (alpha-blending a button
                // background reads as "broken interactable"). Until
                // the title alpha clears the visibility floor above,
                // the whole area short-circuits, so the unfaded
                // button only appears after the black-out.
                let button = super::theme::game_button(
                    ui,
                    "Respawn",
                    super::theme::ButtonKind::Primary,
                    super::theme::MENU_BUTTON_WIDTH,
                );
                if button.clicked() {
                    respawn_requested = true;
                }
                ui.add_space(12.0);
            });
        });

    respawn_requested
}

/// Try to send `ClientMessage::Respawn`. Doesn't touch `MenuState` —
/// the server's `Correction` reply is what clears the splash, handled
/// by the network tick.
pub(super) fn send_respawn(runtime: &mut ClientRuntime) {
    let Some(session) = runtime.session.as_mut() else {
        return;
    };
    let _ = session.send(ClientMessage::Respawn);
}

/// Advance the splash fade timer once per frame and self-clear once
/// the closing fade has fully played out, so the HUD comes back at
/// the moment the screen is fully transparent.
pub(crate) fn tick_death_splash_system(time: Res<Time>, mut menu: ResMut<MenuState>) {
    let dt = time.delta_secs().max(0.0);
    let clear = match menu.death_splash.as_mut() {
        Some(splash) => {
            splash.elapsed += dt;
            if let Some(closing) = splash.closing_elapsed.as_mut() {
                *closing += dt;
                *closing >= CLOSE_FADE_SECS
            } else {
                false
            }
        }
        None => false,
    };
    if clear {
        menu.death_splash = None;
    }
}
