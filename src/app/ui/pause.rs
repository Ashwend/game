use bevy_egui::egui;

use crate::{
    analytics::SessionEndReason,
    app::{
        state::{ClientRuntime, MenuState, SaveStore, Screen, SessionShutdownTasks},
        systems::PendingSessionEndReason,
    },
};

use crate::update::UpdateState;

use super::{
    danger_menu_button, menu_button, modal::backdrop_layer, theme, update::pause_update_row,
};

pub(super) fn pause_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    shutdown_tasks: &mut SessionShutdownTasks,
    store: &SaveStore,
    pending_session_end: &mut PendingSessionEndReason,
    update: &mut UpdateState,
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
                    // Gameplay never actually pauses, the authoritative server
                    // keeps ticking while this overlay is up, so brand it with
                    // the game name in the title typeface rather than a
                    // misleading "Paused". `Extend` keeps it on one line.
                    ui.add(
                        egui::Label::new(theme::title("ASHWEND", 44.0))
                            .wrap_mode(egui::TextWrapMode::Extend),
                    );
                    ui.add_space(16.0);
                    if menu_button(ui, "Resume").clicked() {
                        menu.pause_open = false;
                        menu.pause_options_open = false;
                    }
                    if menu_button(ui, "Options").clicked() {
                        menu.pause_options_open = true;
                    }
                    // Surfaces "Update available / ready" while in-game, since
                    // the corner pill is suppressed over the HUD. Opens the
                    // shared changelog modal.
                    pause_update_row(ui, update);
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

    player_list_panel(ctx, menu, runtime);
}

/// Connected-player roster shown beside the pause menu: each online player's
/// name and ping, with a "Message" button that opens chat pre-filled with a
/// whisper command. Reads `runtime.players` (the server's roster broadcast),
/// which is AoI-independent so it lists everyone, not just nearby players.
fn player_list_panel(ctx: &egui::Context, menu: &mut MenuState, runtime: &ClientRuntime) {
    // Singleplayer is just you on a loopback host, so the roster is noise: only
    // show it for remote (multiplayer) sessions.
    if !runtime.is_multiplayer_session() {
        return;
    }
    if runtime.players.is_empty() {
        return;
    }
    let local_id = runtime.client_id;
    // Clone the small roster so the row loop can mutate `menu` (whisper) without
    // holding an immutable borrow of `runtime` across the closure.
    let players = runtime.players.clone();

    egui::Area::new("pause_player_list".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_CENTER, [28.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(300.0);
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(252.0);
                ui.label(
                    egui::RichText::new(format!("Players online ({})", players.len()))
                        .size(15.0)
                        .strong()
                        .color(theme::text()),
                );
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for entry in &players {
                            player_row(ui, menu, entry, local_id == Some(entry.client_id));
                        }
                    });
            });
        });
}

/// One roster row: ping chip, name, and (for other players) a "Message" button.
fn player_row(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    entry: &crate::protocol::PlayerListEntry,
    is_self: bool,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} ms", entry.ping_ms))
                .size(11.0)
                .monospace()
                .color(ping_color(entry.ping_ms)),
        );
        ui.add_space(6.0);
        let name = if is_self {
            format!("{} (you)", entry.name)
        } else {
            entry.name.clone()
        };
        ui.label(egui::RichText::new(name).color(theme::text()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !is_self {
                let response =
                    theme::compact_button(ui, "Message", theme::ButtonKind::Secondary, 84.0);
                if response.clicked() {
                    theme::record_click_sound(ui, &response);
                    // Hand off to chat with a whisper command pre-filled, then
                    // close the pause overlay so the input is usable.
                    menu.pause_open = false;
                    menu.pause_options_open = false;
                    menu.chat_open = true;
                    menu.chat_focus_pending = true;
                    menu.chat_input = format!("/w {} ", entry.name);
                }
            }
        });
    });
    ui.add_space(3.0);
}

/// Latency color: green for snappy, amber for noticeable, red for laggy.
fn ping_color(ping_ms: u16) -> egui::Color32 {
    if ping_ms < 80 {
        egui::Color32::from_rgb(125, 196, 55)
    } else if ping_ms < 160 {
        egui::Color32::from_rgb(228, 200, 120)
    } else {
        egui::Color32::from_rgb(228, 120, 120)
    }
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
        // The header renders with the custom `cinzel` family; bind it (applied
        // at the next `begin_pass`, which `ctx.run` performs) or layout panics.
        theme::install_title_font(&ctx);
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
        let mut update = UpdateState::idle_for_test();

        let output = ctx.run(raw_input(), |ctx| {
            pause_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &mut shutdown_tasks,
                &store,
                &mut pending_session_end,
                &mut update,
            );
        });

        assert!(output.shapes.len() > 1);
        assert_eq!(menu.screen, Screen::InGame);
        assert!(menu.pause_open);
        assert!(menu.inventory_open);
        assert!(menu.chat_open);
    }
}
