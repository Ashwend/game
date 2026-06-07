use bevy_egui::egui::{self, RichText};

use crate::{
    analytics::{Analytics, ConnectFailReason, Event, events::mask_host},
    app::state::{
        ClientRuntime, CurrentUser, DirectConnectDialog, LoadingSplash, LoadingSplashKind,
        MenuState, Screen,
    },
    net::ClientNetwork,
};

mod connect;

use super::{
    modal,
    theme::{self, ButtonKind},
};

/// Shown until a real server browser ships. There is one official server, so
/// the whole screen is a single confirm-to-join prompt over the menu backdrop.
const JOIN_PROMPT_BODY: &str = "A full server browser is on the way. For now, we host a single \
official server so you can jump in and play with everyone else.\n\nJoin the official server now?";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinChoice {
    Join,
    Cancel,
}

pub(super) fn multiplayer_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    user: &CurrentUser,
    network: &ClientNetwork,
    analytics: &Analytics,
) {
    if let Some(result) = menu
        .direct_connect
        .as_mut()
        .and_then(connect::take_finished)
    {
        connect::finish(menu, runtime, result, analytics);
    }

    let connecting = menu
        .direct_connect
        .as_ref()
        .is_some_and(DirectConnectDialog::is_connecting);
    if connecting {
        ctx.request_repaint();
    }

    handle_multiplayer_escape(ctx, menu);

    match draw_join_prompt(ctx, menu, connecting) {
        Some(JoinChoice::Join) if !connecting => start_join(ctx, menu, user, network, analytics),
        Some(JoinChoice::Cancel) if !connecting => leave_multiplayer(menu),
        _ => {}
    }
}

fn draw_join_prompt(ctx: &egui::Context, menu: &MenuState, connecting: bool) -> Option<JoinChoice> {
    let error = menu
        .direct_connect
        .as_ref()
        .and_then(|dialog| dialog.error.clone());

    let output = modal::modal_shell(
        ctx,
        "multiplayer_join_modal",
        true,
        340.0,
        460.0,
        |ui, choice| {
            ui.label(theme::section("Multiplayer"));
            ui.add_space(12.0);
            ui.label(
                RichText::new(JOIN_PROMPT_BODY)
                    .size(14.0)
                    .color(theme::text()),
            );

            if let Some(error) = &error {
                ui.add_space(8.0);
                ui.label(RichText::new(error).size(13.0).color(theme::error_text()));
            }

            ui.add_space(18.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if connecting {
                    theme::compact_button_with_state(
                        ui,
                        "Join",
                        ButtonKind::Primary,
                        92.0,
                        theme::ButtonState::Loading,
                    );
                } else if theme::compact_button(ui, "Join", ButtonKind::Primary, 92.0).clicked() {
                    *choice = Some(JoinChoice::Join);
                }
                ui.add_enabled_ui(!connecting, |ui| {
                    if theme::compact_button(ui, "Cancel", ButtonKind::Secondary, 92.0).clicked() {
                        *choice = Some(JoinChoice::Cancel);
                    }
                });
            });
        },
    );

    let mut choice = output.choice;
    if choice.is_none() && !connecting && output.confirm_shortcut_pressed {
        choice = Some(JoinChoice::Join);
    }
    if choice.is_none() && !connecting && output.clicked_outside {
        choice = Some(JoinChoice::Cancel);
    }
    choice
}

fn start_join(
    ctx: &egui::Context,
    menu: &mut MenuState,
    user: &CurrentUser,
    network: &ClientNetwork,
    analytics: &Analytics,
) {
    let mut dialog = DirectConnectDialog::new(&menu.multiplayer_addr);
    let display_target = format!("{}:{}", dialog.host, dialog.port);
    analytics.track(Event::ConnectAttempted {
        target_host_masked: mask_host(&display_target),
    });

    match connect::start_attempt(ctx, &mut dialog, user, network) {
        Ok(()) => {
            menu.loading_splash = Some(LoadingSplash::new(
                LoadingSplashKind::JoiningServer,
                display_target,
            ));
        }
        Err(error) => {
            analytics.track(Event::ConnectFailed {
                reason: ConnectFailReason::Other,
            });
            dialog.error = Some(error);
            ctx.request_repaint();
        }
    }

    menu.direct_connect = Some(dialog);
}

fn leave_multiplayer(menu: &mut MenuState) {
    menu.direct_connect = None;
    menu.status = None;
    menu.screen = Screen::MainMenu;
}

fn handle_multiplayer_escape(ctx: &egui::Context, menu: &mut MenuState) {
    if !ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        return;
    }

    if menu
        .direct_connect
        .as_ref()
        .is_some_and(DirectConnectDialog::is_connecting)
    {
        ctx.request_repaint();
        return;
    }

    leave_multiplayer(menu);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::DirectConnectAttempt;
    use std::sync::mpsc;

    fn raw_input_with_events(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 768.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn escape_event() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    #[test]
    fn escape_returns_to_main_menu_when_idle() {
        let mut menu = MenuState {
            screen: Screen::Multiplayer,
            ..Default::default()
        };
        let ctx = egui::Context::default();

        let _ = ctx.run(raw_input_with_events(vec![escape_event()]), |ctx| {
            handle_multiplayer_escape(ctx, &mut menu);
        });

        assert_eq!(menu.screen, Screen::MainMenu);
        assert!(menu.direct_connect.is_none());
    }

    #[test]
    fn escape_is_ignored_while_connecting() {
        let (_tx, receiver) = mpsc::channel();
        let mut menu = MenuState {
            screen: Screen::Multiplayer,
            direct_connect: Some(DirectConnectDialog {
                host: "127.0.0.1".to_owned(),
                port: "7777".to_owned(),
                error: None,
                attempt: Some(DirectConnectAttempt {
                    receiver: std::sync::Mutex::new(receiver),
                }),
            }),
            ..Default::default()
        };
        let ctx = egui::Context::default();

        let _ = ctx.run(raw_input_with_events(vec![escape_event()]), |ctx| {
            handle_multiplayer_escape(ctx, &mut menu);
        });

        assert_eq!(menu.screen, Screen::Multiplayer);
        assert!(
            menu.direct_connect.is_some(),
            "the in-flight attempt must survive an escape press"
        );
    }
}
