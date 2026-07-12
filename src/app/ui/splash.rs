use bevy_egui::egui;

use crate::app::state::{LoadingSplash, LoadingSplashKind, MenuBackdropVisibility, MenuState};

use super::theme;

/// Renders the loading splash overlay on top of every other screen.
/// Ticks the splash timer using the supplied frame `delta_seconds`, drops
/// the splash when it has fully faded out, and flips `ready` once the splash
/// is allowed to fade:
/// - `Startup` waits for the menu backdrop to finish warming up so the splash
///   and the backdrop crossfade as one motion.
/// - World-entry splashes (`EnteringWorld` / `JoiningServer`) wait for
///   `world_ready`, the joined world being applied, spawned, and replicated,
///   so the reveal lands on a populated, rendered scene rather than an empty
///   frame. See [`MenuState`]'s `enter_in_game` and `LoadingSplash::note_world_ready`.
pub(super) fn loading_splash_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    backdrop_visibility: &MenuBackdropVisibility,
    world_ready: bool,
    world_backlog: usize,
    delta_seconds: f32,
) {
    let alpha = {
        let Some(splash) = menu.loading_splash.as_mut() else {
            return;
        };
        if splash.kind == LoadingSplashKind::Startup
            && !splash.ready
            && backdrop_visibility.has_finished_warmup()
        {
            splash.ready = true;
        }
        // World-entry splashes gate their fade on the world being ready
        // (no-op for `Startup` and once already ready).
        splash.note_world_ready(world_ready);
        let Some(alpha) = splash.tick(delta_seconds) else {
            menu.loading_splash = None;
            ctx.request_repaint();
            return;
        };
        alpha
    };

    // While the splash is on-screen the underlying work is either still
    // running or we're holding the minimum-display window, repaint every
    // frame so the spinner spins and the fade animates smoothly.
    ctx.request_repaint();

    let splash = menu
        .loading_splash
        .as_ref()
        .expect("splash present after tick");

    draw_overlay(ctx, splash, alpha, world_backlog);
}

fn draw_overlay(ctx: &egui::Context, splash: &LoadingSplash, alpha: u8, world_backlog: usize) {
    let rect = ctx.content_rect();
    let fill = egui::Color32::from_rgba_unmultiplied(2, 4, 7, alpha);
    egui::Area::new(egui::Id::new("loading_splash"))
        // Tooltip order so it sits above every other panel/area, including
        // any dialog that might still be visible at the moment we flip
        // screens.
        .order(egui::Order::Tooltip)
        .interactable(true)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            let local_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, rect.size());
            // Swallow clicks so they don't fall through to the screen
            // underneath while the overlay is up.
            ui.allocate_rect(local_rect, egui::Sense::click_and_drag());
            ui.painter().rect_filled(local_rect, 0, fill);

            draw_panel(ui, splash, rect, alpha, world_backlog);
        });
}

fn draw_panel(
    ui: &mut egui::Ui,
    splash: &LoadingSplash,
    screen_rect: egui::Rect,
    alpha: u8,
    world_backlog: usize,
) {
    let center = egui::pos2(screen_rect.width() * 0.5, screen_rect.height() * 0.5 - 28.0);
    let title = splash.title();
    let subtitle = splash.target_label.trim();

    let title_color = with_alpha(theme::text(), alpha);
    let subtitle_color = with_alpha(
        egui::Color32::from_rgb(190, 206, 224),
        scale_alpha(alpha, 0.82),
    );
    let hint_color = with_alpha(
        egui::Color32::from_rgb(150, 168, 188),
        scale_alpha(alpha, 0.7),
    );

    ui.painter().text(
        center,
        egui::Align2::CENTER_BOTTOM,
        title,
        egui::FontId::new(28.0, egui::FontFamily::Proportional),
        title_color,
    );

    if !subtitle.is_empty() {
        ui.painter().text(
            center + egui::vec2(0.0, 12.0),
            egui::Align2::CENTER_TOP,
            subtitle,
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
            subtitle_color,
        );
    }

    let spinner_center = center + egui::vec2(0.0, 64.0);
    let spinner_rect = egui::Rect::from_center_size(spinner_center, egui::vec2(28.0, 28.0));
    let mut spinner_ui = ui.new_child(egui::UiBuilder::new().max_rect(spinner_rect).layout(
        egui::Layout::centered_and_justified(egui::Direction::TopDown),
    ));
    spinner_ui
        .style_mut()
        .visuals
        .widgets
        .noninteractive
        .fg_stroke
        .color = with_alpha(egui::Color32::from_rgb(170, 196, 224), alpha);
    spinner_ui.add(egui::Spinner::new().size(24.0));

    let hint = match splash.kind {
        LoadingSplashKind::Startup => "Preparing main menu…",
        LoadingSplashKind::EnteringWorld => "Preparing your world…",
        LoadingSplashKind::JoiningServer => "Establishing connection…",
    };
    ui.painter().text(
        spinner_center + egui::vec2(0.0, 36.0),
        egui::Align2::CENTER_TOP,
        hint,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        hint_color,
    );

    // World-entry progress: while the readiness gate holds the splash, show
    // a STEADY status line. It renders every frame until the splash is ready
    // (only the text varies), because the spawn queue empties and refills
    // between replication packets; keying visibility on `backlog > 0` made
    // the line blink in and out.
    let world_entry = matches!(
        splash.kind,
        LoadingSplashKind::EnteringWorld | LoadingSplashKind::JoiningServer
    );
    if world_entry && !splash.ready {
        let progress = if world_backlog > 0 {
            format!("Placing {world_backlog} objects…")
        } else {
            "Settling the world…".to_owned()
        };
        ui.painter().text(
            spinner_center + egui::vec2(0.0, 56.0),
            egui::Align2::CENTER_TOP,
            progress,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
            with_alpha(
                egui::Color32::from_rgb(130, 148, 168),
                scale_alpha(alpha, 0.6),
            ),
        );
    }
}

fn with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
    let [r, g, b, a] = color.to_array();
    let combined = ((u32::from(a) * u32::from(alpha)) / u32::from(u8::MAX)) as u8;
    egui::Color32::from_rgba_unmultiplied(r, g, b, combined)
}

fn scale_alpha(alpha: u8, factor: f32) -> u8 {
    (f32::from(alpha) * factor.clamp(0.0, 1.0)).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::LoadingSplashKind;

    #[test]
    fn splash_stays_visible_until_ready_and_then_holds_min_duration_before_fading() {
        let mut splash = LoadingSplash::new(LoadingSplashKind::EnteringWorld, "World");

        // Not ready yet: full alpha no matter how long.
        for _ in 0..20 {
            assert_eq!(splash.tick(0.02), Some(u8::MAX));
        }

        // A fast load that finishes well inside the hold window: splash
        // must continue at full alpha for the rest of the hold window
        // before fade-out begins.
        let mut fast = LoadingSplash::new(LoadingSplashKind::JoiningServer, "127.0.0.1");
        fast.ready = true;
        assert_eq!(fast.tick(0.05), Some(u8::MAX));
        assert_eq!(fast.tick(0.1), Some(u8::MAX));

        // Total hold + fade window is bounded, splash must drop within 3s.
        let mut faded = false;
        for _ in 0..200 {
            if fast.tick(0.05).is_none() {
                faded = true;
                break;
            }
        }
        assert!(faded, "splash should drop after hold + fade window");
    }

    #[test]
    fn splash_fades_immediately_when_load_took_longer_than_min_hold() {
        let mut splash = LoadingSplash::new(LoadingSplashKind::EnteringWorld, "Slow World");
        // Long load that exceeds the min-hold window before the world is
        // ready: the player has already been looking at the splash longer
        // than min, so once ready, fade can start straight away.
        for _ in 0..40 {
            let _ = splash.tick(0.1);
        }
        splash.ready = true;
        // Fade should complete within `FADE_SECONDS` from here.
        let mut faded = false;
        for _ in 0..30 {
            if splash.tick(0.05).is_none() {
                faded = true;
                break;
            }
        }
        assert!(faded);
    }

    #[test]
    fn startup_splash_flips_ready_once_backdrop_warmup_completes() {
        let ctx = egui::Context::default();
        let mut menu = MenuState::default();
        let mut backdrop = MenuBackdropVisibility::default();
        // Warm the backdrop past its blur warmup window before the first
        // splash tick so the readiness signal is true.
        for _ in 0..40 {
            let _ = backdrop.cover_alpha(crate::app::state::Screen::MainMenu, 0.1);
        }
        assert!(backdrop.has_finished_warmup());

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| loading_splash_ui(ctx, &mut menu, &backdrop, false, 0, 0.05),
        );
        assert!(menu.loading_splash.as_ref().expect("startup splash").ready);
    }

    #[test]
    fn splash_overlay_renders_in_headless_context() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            loading_splash: Some(LoadingSplash::new(
                LoadingSplashKind::JoiningServer,
                "127.0.0.1:7777",
            )),
            ..Default::default()
        };
        let backdrop = MenuBackdropVisibility::default();

        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| loading_splash_ui(ctx, &mut menu, &backdrop, false, 0, 0.016),
        );

        assert!(!output.shapes.is_empty());
        assert!(menu.loading_splash.is_some());
    }
}
