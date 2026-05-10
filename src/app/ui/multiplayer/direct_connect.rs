use std::{
    net::SocketAddr,
    sync::mpsc::{self, TryRecvError},
    thread,
};

use anyhow::Result;
use bevy_egui::egui;

use crate::{
    app::state::{
        ClientRuntime, DirectConnectAttempt, DirectConnectDialog, DirectConnectResult, MenuState,
        Screen, SteamUser,
    },
    net::ClientSession,
};

use self::target::{DirectConnectTarget, direct_connect_target, resolve_direct_connect_target};
use super::super::{
    modal,
    theme::{self, ButtonKind},
};

mod target;

const DIRECT_CONNECT_HOST_INPUT_ID: &str = "direct_connect_host_input";
const DIRECT_CONNECT_PORT_INPUT_ID: &str = "direct_connect_port_input";
const DIRECT_CONNECT_FIELD_HEIGHT: f32 = 34.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectConnectChoice {
    Connect,
    Cancel,
}

#[derive(Debug, Clone, Copy)]
struct DirectConnectModalOutput {
    choice: Option<DirectConnectChoice>,
    finished_closing: bool,
}

pub(super) fn direct_connect_dialog_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    user: &SteamUser,
) {
    let connect_result = {
        let Some(dialog) = menu.direct_connect.as_mut() else {
            return;
        };

        let result = take_finished_direct_connect(dialog);
        if dialog.is_connecting() {
            ctx.request_repaint();
        }
        result
    };

    if let Some(result) = connect_result {
        finish_direct_connect(menu, runtime, result);
    }

    let finished_closing;
    {
        let Some(dialog) = menu.direct_connect.as_mut() else {
            return;
        };

        let output = direct_connect_modal(ctx, dialog, !dialog.closing);
        if let Some(choice) = output.choice {
            match choice {
                DirectConnectChoice::Connect => match direct_connect_target(dialog) {
                    Ok(target) => {
                        if let Err(error) = start_direct_connect_attempt(ctx, dialog, target, user)
                        {
                            dialog.error = Some(error);
                            ctx.request_repaint();
                        }
                    }
                    Err(error) => {
                        dialog.error = Some(error.to_string());
                        ctx.request_repaint();
                    }
                },
                DirectConnectChoice::Cancel => {
                    dialog.closing = true;
                    ctx.request_repaint();
                }
            }
        }
        finished_closing = output.finished_closing;
    }

    if finished_closing {
        menu.direct_connect = None;
    }
}

fn direct_connect_modal(
    ctx: &egui::Context,
    dialog: &mut DirectConnectDialog,
    open: bool,
) -> DirectConnectModalOutput {
    let output = modal::modal_shell(
        ctx,
        "direct_connect_modal",
        open,
        340.0,
        440.0,
        |ui, choice| {
            draw_direct_connect_form(ui, dialog, choice);
        },
    );

    let mut choice = output.choice;
    let connecting = dialog.is_connecting();
    if choice.is_none() && !connecting && output.confirm_shortcut_pressed {
        choice = Some(DirectConnectChoice::Connect);
    }
    if choice.is_none() && !connecting && output.clicked_outside {
        choice = Some(DirectConnectChoice::Cancel);
    }

    DirectConnectModalOutput {
        choice,
        finished_closing: output.finished_closing,
    }
}

fn draw_direct_connect_form(
    ui: &mut egui::Ui,
    dialog: &mut DirectConnectDialog,
    choice: &mut Option<DirectConnectChoice>,
) {
    let connecting = dialog.is_connecting();
    ui.label(theme::section("Direct Connect"));
    ui.add_space(12.0);

    ui.add_enabled_ui(!connecting, |ui| {
        ui.label(theme::field_label("Server Address"));
        ui.add_sized(
            [ui.available_width(), DIRECT_CONNECT_FIELD_HEIGHT],
            theme::text_input(&mut dialog.host).id(egui::Id::new(DIRECT_CONNECT_HOST_INPUT_ID)),
        );

        ui.add_space(6.0);
        ui.label(theme::field_label("Port"));
        ui.add_sized(
            [ui.available_width(), DIRECT_CONNECT_FIELD_HEIGHT],
            theme::text_input(&mut dialog.port).id(egui::Id::new(DIRECT_CONNECT_PORT_INPUT_ID)),
        );
    });

    if let Some(error) = &dialog.error {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(error)
                .size(13.0)
                .color(egui::Color32::from_rgb(255, 154, 130)),
        );
    }

    ui.add_space(18.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if connecting {
            theme::compact_button_with_state(
                ui,
                "Connect",
                ButtonKind::Primary,
                92.0,
                theme::ButtonState::Loading,
            );
        } else if theme::compact_button(ui, "Connect", ButtonKind::Primary, 92.0).clicked() {
            *choice = Some(DirectConnectChoice::Connect);
        }
        ui.add_enabled_ui(!connecting, |ui| {
            if theme::compact_button(ui, "Cancel", ButtonKind::Secondary, 92.0).clicked() {
                *choice = Some(DirectConnectChoice::Cancel);
            }
        });
    });
}

fn start_direct_connect_attempt(
    ctx: &egui::Context,
    dialog: &mut DirectConnectDialog,
    target: DirectConnectTarget,
    user: &SteamUser,
) -> std::result::Result<(), String> {
    let (tx, receiver) = mpsc::channel::<DirectConnectResult>();
    let user = user.0.clone();
    thread::Builder::new()
        .name("direct-connect-attempt".to_owned())
        .spawn(move || {
            let result = connect_to_target(target, user).map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        })
        .map_err(|error| format!("Could not start connection attempt: {error}"))?;

    dialog.error = None;
    dialog.attempt = Some(DirectConnectAttempt {
        receiver: std::sync::Mutex::new(receiver),
    });
    ctx.request_repaint();
    Ok(())
}

fn connect_to_target(
    target: DirectConnectTarget,
    user: crate::steam::AuthenticatedUser,
) -> Result<(SocketAddr, ClientSession)> {
    let addr = resolve_direct_connect_target(&target)?;
    let session = ClientSession::connect(addr, &user)?;
    Ok((addr, session))
}

fn take_finished_direct_connect(dialog: &mut DirectConnectDialog) -> Option<DirectConnectResult> {
    enum AttemptPoll {
        Result(std::result::Result<DirectConnectResult, TryRecvError>),
        Poisoned,
    }

    let attempt = dialog.attempt.as_ref()?;
    let poll = match attempt.receiver.lock() {
        Ok(receiver) => AttemptPoll::Result(receiver.try_recv()),
        Err(_) => AttemptPoll::Poisoned,
    };

    match poll {
        AttemptPoll::Poisoned => {
            dialog.attempt = None;
            Some(Err("Connection attempt state is unavailable.".to_owned()))
        }
        AttemptPoll::Result(Ok(result)) => {
            dialog.attempt = None;
            Some(result)
        }
        AttemptPoll::Result(Err(TryRecvError::Empty)) => None,
        AttemptPoll::Result(Err(TryRecvError::Disconnected)) => {
            dialog.attempt = None;
            Some(Err(
                "Connection attempt ended before returning a result.".to_owned()
            ))
        }
    }
}

fn finish_direct_connect(
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    result: DirectConnectResult,
) {
    match result {
        Ok((addr, session)) => {
            runtime.start_session(session, None);
            menu.multiplayer_addr = addr.to_string();
            menu.direct_connect = None;
            menu.screen = Screen::InGame;
            menu.pause_open = false;
            menu.pause_options_open = false;
            menu.chat_open = false;
            menu.chat_focus_pending = false;
            menu.status = None;
        }
        Err(error) => {
            if let Some(dialog) = menu.direct_connect.as_mut() {
                dialog.error = Some(format!("Connection failed: {error}"));
            } else {
                menu.status = Some(format!("Connection failed: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn key_press(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    #[test]
    fn escape_does_not_close_direct_connect_modal_while_connecting() {
        let (_tx, receiver) = mpsc::channel::<DirectConnectResult>();
        let mut menu = MenuState {
            screen: Screen::Multiplayer,
            direct_connect: Some(DirectConnectDialog {
                host: "127.0.0.1".to_owned(),
                port: "7777".to_owned(),
                error: None,
                closing: false,
                attempt: Some(DirectConnectAttempt {
                    receiver: std::sync::Mutex::new(receiver),
                }),
            }),
            ..Default::default()
        };
        let ctx = egui::Context::default();

        let _ = ctx.run(
            raw_input_with_events(vec![key_press(egui::Key::Escape)]),
            |ctx| super::super::handle_multiplayer_escape(ctx, &mut menu),
        );

        assert_eq!(menu.screen, Screen::Multiplayer);
        let dialog = menu
            .direct_connect
            .expect("dialog should remain open while connecting");
        assert!(!dialog.closing);
    }
}
