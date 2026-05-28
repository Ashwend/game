use bevy_egui::egui;

use crate::{
    analytics::SessionEndReason,
    app::{
        state::{ClientRuntime, MenuState, SaveStore, Screen, SessionShutdownTasks},
        systems::PendingSessionEndReason,
    },
};

use super::{danger_menu_button, menu_button, modal::backdrop_layer, theme};

pub(super) fn pause_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    shutdown_tasks: &mut SessionShutdownTasks,
    store: &SaveStore,
    pending_session_end: &mut PendingSessionEndReason,
) {
    let backdrop_response = backdrop_layer(
        ctx,
        "pause_backdrop",
        egui::Order::Middle,
        theme::backdrop_color(),
    );

    if backdrop_response.clicked() {
        menu.pause_open = false;
        menu.pause_options_open = false;
    }

    egui::Area::new("pause_menu".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(320.0);
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(272.0);
                ui.vertical_centered(|ui| {
                    ui.label(theme::section("Paused"));
                    ui.add_space(16.0);
                    if menu_button(ui, "Resume").clicked() {
                        menu.pause_open = false;
                        menu.pause_options_open = false;
                    }
                    if menu_button(ui, "Options").clicked() {
                        menu.pause_options_open = true;
                    }
                    if danger_menu_button(ui, "Quit").clicked() {
                        pending_session_end.0 = Some(SessionEndReason::UserQuit);
                        runtime.shutdown_in_background(store.0.clone(), shutdown_tasks);
                        menu.screen = Screen::MainMenu;
                        menu.pause_open = false;
                        menu.pause_options_open = false;
                        menu.inventory_open = false;
                        menu.chat_open = false;
                        menu.chat_focus_pending = false;
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::WorldStore;

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 768.0),
            )),
            ..Default::default()
        }
    }

    #[test]
    fn pause_menu_renders_without_changing_state_when_idle() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            inventory_open: true,
            chat_open: true,
            chat_focus_pending: true,
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let mut shutdown_tasks = SessionShutdownTasks::default();
        let store = SaveStore(WorldStore::new(
            std::env::temp_dir().join(format!("game-pause-test-{}", uuid::Uuid::new_v4())),
        ));
        let mut pending_session_end = PendingSessionEndReason::default();

        let output = ctx.run(raw_input(), |ctx| {
            pause_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &mut shutdown_tasks,
                &store,
                &mut pending_session_end,
            );
        });

        assert!(output.shapes.len() > 1);
        assert_eq!(menu.screen, Screen::InGame);
        assert!(menu.pause_open);
        assert!(menu.inventory_open);
        assert!(menu.chat_open);
    }
}
