mod chat;
mod confirm;
mod hud;
mod menu;
mod modal;
mod multiplayer;
mod pause;
mod theme;
mod worlds;

use bevy::diagnostic::DiagnosticsStore;
use bevy::{app::AppExit, prelude::*};
use bevy_egui::{EguiContexts, egui};

use self::{
    chat::chat_ui,
    confirm::confirmation_ui,
    hud::hud_ui,
    menu::main_menu_ui,
    multiplayer::multiplayer_ui,
    pause::pause_ui,
    theme::{ButtonKind, game_button},
    worlds::worlds_ui,
};
use super::state::{
    ClientRuntime, MenuBackdropVisibility, MenuState, SaveStore, Screen, SteamUser,
};

pub(crate) fn ui_system(
    mut contexts: EguiContexts,
    mut menu: ResMut<MenuState>,
    mut backdrop_visibility: ResMut<MenuBackdropVisibility>,
    mut runtime: ResMut<ClientRuntime>,
    store: Res<SaveStore>,
    user: Res<SteamUser>,
    time: Option<Res<Time>>,
    diagnostics: Res<DiagnosticsStore>,
    mut app_exit: MessageWriter<AppExit>,
) -> bevy::prelude::Result {
    let ctx = contexts.ctx_mut()?;
    theme::apply_game_style(ctx);
    let delta_seconds = time
        .as_ref()
        .map(|time| time.delta_secs())
        .unwrap_or(1.0 / 60.0);
    let cover_alpha = backdrop_visibility.cover_alpha(menu.screen, delta_seconds);
    theme::backdrop_cover(ctx, cover_alpha);

    match menu.screen {
        Screen::MainMenu => main_menu_ui(ctx, &mut menu, &store, &user, &mut app_exit),
        Screen::Worlds => worlds_ui(ctx, &mut menu, &mut runtime, &store, &user),
        Screen::Multiplayer => multiplayer_ui(ctx, &mut menu, &mut runtime, &user),
        Screen::InGame => {
            hud_ui(ctx, &runtime, &diagnostics);
            chat_ui(ctx, &mut menu, &mut runtime);
            if menu.pause_open {
                pause_ui(ctx, &mut menu, &mut runtime, &store);
            }
        }
    }

    confirmation_ui(ctx, &mut menu, &store);

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
