use bevy_egui::egui;

use crate::{
    app::state::{ConfirmationDialog, CurrentUser, MenuBackdropTime, MenuState, SaveStore, Screen},
    protocol::GAME_VERSION,
    update::UpdateState,
    util::open_url,
};

use super::{
    danger_menu_button, primary_menu_button,
    theme::{self, MENU_BUTTON_WIDTH, MENU_WIDTH},
};
// The Singleplayer entry (and the world-list refresh it triggers) is gated to
// dev/test builds; see `main_menu_ui`. Keep the import on the same gate so it is
// not flagged unused in shipped release builds.
#[cfg(debug_assertions)]
use super::worlds::refresh_worlds;

/// Community Discord invite. Same link the website uses (see
/// `website/src/lib/config.ts`); keep them in sync if it ever rotates.
const DISCORD_INVITE_URL: &str = "https://discord.gg/gVqTumNb8b";

// `store` feeds only the dev/test Singleplayer entry below; in release builds
// that entry is compiled out, leaving the parameter unused.
#[cfg_attr(not(debug_assertions), allow(unused_variables))]
pub(super) fn main_menu_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    store: &SaveStore,
    user: &CurrentUser,
    update: &mut UpdateState,
    // Backdrop-time scrubber state + its Dev-tab visibility gate. Both feed only
    // the debug-only slider below, so they are unused in release builds.
    backdrop_time: &mut MenuBackdropTime,
    show_backdrop_time_slider: bool,
) {
    theme::screen_scrim(ctx, "main_menu_scrim", 118);
    egui::Area::new("main_menu".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -20.0])
        .show(ctx, |ui| {
            ui.set_width(MENU_WIDTH);
            ui.vertical_centered(|ui| {
                // The title font is wider than the menu column, so let it
                // extend onto a single centred line instead of wrapping.
                ui.add(
                    egui::Label::new(theme::title("ASHWEND", 78.0))
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
                ui.add_space(20.0);
                let panel = theme::panel_frame().inner_margin(egui::Margin::same(24));
                panel.show(ui, |ui| {
                    ui.set_width(MENU_BUTTON_WIDTH);
                    ui.vertical_centered(|ui| {
                        // Singleplayer runs an in-process loopback host: handy
                        // for local iteration, but not a shipped way to play.
                        // Gate the entry out of release builds (debug_assertions
                        // is off only in the --release publish/CI path), leaving
                        // players Multiplayer / Options / Quit. The worlds screen
                        // it opens stays compiled and fully usable in any dev/test
                        // build; only this entry point disappears from release.
                        #[cfg(debug_assertions)]
                        if primary_menu_button(ui, "Singleplayer").clicked() {
                            refresh_worlds(menu, store);
                            menu.screen = Screen::Worlds;
                        }
                        if primary_menu_button(ui, "Multiplayer").clicked() {
                            menu.screen = Screen::Multiplayer;
                        }
                        if super::menu_button(ui, "Options").clicked() {
                            menu.screen = Screen::Options;
                        }
                        if danger_menu_button(ui, "Quit").clicked() {
                            menu.quit_requested = true;
                        }
                    });
                });

                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new(format!("Signed in as {}", user.0.display_name))
                        .color(theme::text()),
                );
                ui.add_space(8.0);
                if ui.link(theme::muted("Sign out")).clicked() {
                    menu.confirmation = Some(ConfirmationDialog::sign_out());
                }
                if let Some(status) = &menu.status {
                    ui.add_space(4.0);
                    ui.label(theme::status_text(status));
                }
            });
        });
    draw_version_indicator(ctx, update);
    draw_discord_link(ctx);
    #[cfg(debug_assertions)]
    if show_backdrop_time_slider {
        draw_backdrop_time_slider(ctx, backdrop_time);
    }
}

/// Scrubber for the menu backdrop's pinned time of day. Drag to sweep the
/// title-screen sky live (it updates immediately via `MenuBackdropTime`, read by
/// `scene::sky`), read off the `HH:MM`, then bake the value into
/// `MENU_BACKDROP_SECONDS`. Gated behind the debug-only Dev options tab toggle
/// (`settings.dev.backdrop_time_slider`, off by default) and compiled out of
/// release builds, so shipped players never see it.
#[cfg(debug_assertions)]
fn draw_backdrop_time_slider(ctx: &egui::Context, backdrop_time: &mut MenuBackdropTime) {
    use crate::world_time::WorldTime;

    egui::Window::new("Backdrop time (dev)")
        .order(egui::Order::Foreground)
        .default_pos([18.0, 18.0])
        .resizable(false)
        .show(ctx, |ui| {
            // Edit in whole minutes so the value lands on clean HH:MM
            // boundaries instead of accumulating float noise.
            let mut minutes = (backdrop_time.seconds_of_day / 60.0).round();
            let response = ui.add(
                egui::Slider::new(&mut minutes, 0.0..=1439.0)
                    .step_by(1.0)
                    .show_value(false),
            );
            if response.changed() {
                backdrop_time.seconds_of_day = minutes * 60.0;
            }

            let time = WorldTime {
                seconds_of_day: backdrop_time.seconds_of_day,
                multiplier: 0.0,
            };
            let hours = backdrop_time.seconds_of_day / 3600.0;
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(time.format_hhmm())
                    .color(theme::accent())
                    .size(22.0),
            );
            ui.label(theme::muted(format!(
                "MENU_BACKDROP_SECONDS = {hours:.4} * 3600.0"
            )));
        });
}

/// Persistent "Join our Discord" affordance in the bottom-left corner of the
/// title screen, balancing the version indicator on the right. Opens the same
/// invite the website uses in the system browser.
fn draw_discord_link(ctx: &egui::Context) {
    egui::Area::new("main_menu_discord".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_BOTTOM, [18.0, -14.0])
        .show(ctx, |ui| {
            let link = ui
                .link(egui::RichText::new("Join our Discord").color(theme::accent()))
                .on_hover_text("Opens discord.gg in your browser");
            if link.clicked() {
                let _ = open_url(DISCORD_INVITE_URL);
            }
        });
}

/// Bottom-right version label. White (not muted) and clickable: opens the
/// "what's new in this version" changelog modal so players can see what shipped
/// in the build they're on without leaving the game.
fn draw_version_indicator(ctx: &egui::Context, update: &mut UpdateState) {
    egui::Area::new("main_menu_version".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, [-18.0, -14.0])
        .show(ctx, |ui| {
            let label = egui::RichText::new(format!("v{GAME_VERSION}")).color(egui::Color32::WHITE);
            if ui
                .link(label)
                .on_hover_text("See what's new in this version")
                .clicked()
            {
                update.open_current_changelog();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth::AuthenticatedUser, save::WorldStore};

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 768.0),
            )),
            ..Default::default()
        }
    }

    fn store() -> SaveStore {
        SaveStore(WorldStore::new(
            std::env::temp_dir().join(format!("game-main-menu-test-{}", uuid::Uuid::new_v4())),
        ))
    }

    fn user() -> CurrentUser {
        CurrentUser(AuthenticatedUser {
            account_id: 1,
            display_name: "Tester".to_owned(),
            token: String::new(),
        })
    }

    #[test]
    fn main_menu_renders_status_and_version() {
        let ctx = egui::Context::default();
        // The title renders with the custom `cinzel` family; bind it (applied
        // at the next `begin_pass`, which `ctx.run` performs) or layout panics.
        theme::install_title_font(&ctx);
        let mut menu = MenuState {
            status: Some("Ready".to_owned()),
            ..Default::default()
        };
        let store = store();
        let user = user();
        let mut update = UpdateState::idle_for_test();
        let mut backdrop_time = MenuBackdropTime::default();

        let output = ctx.run(raw_input(), |ctx| {
            main_menu_ui(
                ctx,
                &mut menu,
                &store,
                &user,
                &mut update,
                &mut backdrop_time,
                // Exercise the backdrop-time slider render path in debug builds.
                true,
            );
        });

        assert!(output.shapes.len() > 1);
        assert_eq!(menu.screen, Screen::MainMenu);
        assert!(!menu.quit_requested);
    }
}
