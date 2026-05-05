mod chat;
mod hud;
mod menu;
mod multiplayer;
mod pause;
mod theme;
mod worlds;

use bevy::{app::AppExit, prelude::*};
use bevy_egui::{EguiContexts, egui};

use self::{
    chat::chat_ui,
    hud::hud_ui,
    menu::main_menu_ui,
    multiplayer::multiplayer_ui,
    pause::pause_ui,
    theme::{ButtonKind, game_button},
    worlds::worlds_ui,
};
use super::state::{ClientRuntime, MenuState, SaveStore, Screen, SteamUser};

pub(crate) fn ui_system(
    mut contexts: EguiContexts,
    mut menu: ResMut<MenuState>,
    mut runtime: ResMut<ClientRuntime>,
    store: Res<SaveStore>,
    user: Res<SteamUser>,
    mut app_exit: MessageWriter<AppExit>,
) -> bevy::prelude::Result {
    let ctx = contexts.ctx_mut()?;
    theme::apply_game_style(ctx);

    match menu.screen {
        Screen::MainMenu => main_menu_ui(ctx, &mut menu, &store, &user, &mut app_exit),
        Screen::Worlds => worlds_ui(ctx, &mut menu, &mut runtime, &store, &user),
        Screen::Multiplayer => multiplayer_ui(ctx, &mut menu, &mut runtime, &user),
        Screen::InGame => {
            hud_ui(ctx, &runtime);
            chat_ui(ctx, &mut menu, &mut runtime);
            if menu.pause_open {
                pause_ui(ctx, &mut menu, &mut runtime, &store);
            }
        }
    }

    Ok(())
}

fn menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    game_button(ui, text, ButtonKind::Secondary, 260.0)
}

fn primary_menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    game_button(ui, text, ButtonKind::Primary, 260.0)
}

fn danger_menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    game_button(ui, text, ButtonKind::Danger, 260.0)
}
