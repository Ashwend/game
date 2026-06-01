use bevy_egui::egui;

use crate::{
    app::state::{CurrentUser, MenuState, SaveStore, Screen},
    protocol::GAME_VERSION,
};

use super::{
    danger_menu_button, primary_menu_button,
    theme::{self, MENU_BUTTON_WIDTH, MENU_WIDTH},
    worlds::refresh_worlds,
};

pub(super) fn main_menu_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    store: &SaveStore,
    user: &CurrentUser,
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
                    menu.sign_out_requested = true;
                }
                if let Some(status) = &menu.status {
                    ui.add_space(4.0);
                    ui.label(theme::status_text(status));
                }
            });
        });
    draw_version_indicator(ctx);
}

fn draw_version_indicator(ctx: &egui::Context) {
    egui::Area::new("main_menu_version".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, [-18.0, -14.0])
        .show(ctx, |ui| {
            ui.label(theme::muted(format!("v{GAME_VERSION}")));
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

        let output = ctx.run(raw_input(), |ctx| {
            main_menu_ui(ctx, &mut menu, &store, &user);
        });

        assert!(output.shapes.len() > 1);
        assert_eq!(menu.screen, Screen::MainMenu);
        assert!(!menu.quit_requested);
    }
}
