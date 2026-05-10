use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::mpsc::{self, TryRecvError},
    thread,
};

use anyhow::{Result, bail};
use bevy_egui::egui;

use crate::{
    app::state::{
        ClientRuntime, DirectConnectAttempt, DirectConnectDialog, DirectConnectResult, MenuState,
        Screen, SteamUser,
    },
    net::ClientSession,
    steam::{OfflineSteamBackend, SteamBackend},
};

use super::{
    modal,
    theme::{self, ButtonKind},
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectConnectTarget {
    host: String,
    port: u16,
}

pub(super) fn multiplayer_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    user: &SteamUser,
) {
    theme::screen_scrim(ctx, "multiplayer_scrim", 145);
    handle_multiplayer_escape(ctx, menu);
    theme::anchored_panel(
        ctx,
        "multiplayer_panel",
        560.0,
        egui::Align2::CENTER_CENTER,
        [0.0, -10.0],
        |ui| {
            draw_multiplayer_header(ui, menu);

            ui.add_space(16.0);
            theme::inset_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical(|ui| {
                    ui.label(theme::field_label("Steam"));
                    if theme::game_button(
                        ui,
                        "Open Server Browser",
                        ButtonKind::Primary,
                        ui.available_width(),
                    )
                    .clicked()
                    {
                        let steam = OfflineSteamBackend;
                        menu.status = match steam.open_server_browser() {
                            Ok(()) => Some("opened Steam server browser".to_owned()),
                            Err(error) => Some(format!("Steam browser unavailable: {error}")),
                        };
                    }
                });
            });

            if let Some(status) = &menu.status {
                ui.add_space(10.0);
                ui.label(theme::status_text(status));
            }
        },
    );
    direct_connect_dialog_ui(ctx, menu, runtime, user);
}

fn draw_multiplayer_header(ui: &mut egui::Ui, menu: &mut MenuState) {
    if ui.available_width() < 340.0 {
        ui.label(theme::section("Multiplayer"));
        ui.add_space(4.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            draw_header_buttons(ui, menu);
        });
        return;
    }

    ui.horizontal(|ui| {
        ui.label(theme::section("Multiplayer"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            draw_header_buttons(ui, menu);
        });
    });
}

fn draw_header_buttons(ui: &mut egui::Ui, menu: &mut MenuState) {
    if theme::compact_button(ui, "Back", ButtonKind::Secondary, 78.0).clicked() {
        menu.screen = Screen::MainMenu;
    }
    if theme::compact_button(ui, "Direct Connect", ButtonKind::Primary, 128.0).clicked() {
        menu.direct_connect = Some(DirectConnectDialog::new(&menu.multiplayer_addr));
    }
}

fn handle_multiplayer_escape(ctx: &egui::Context, menu: &mut MenuState) {
    if !ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        return;
    }

    if let Some(dialog) = menu.direct_connect.as_mut() {
        if dialog.is_connecting() {
            ctx.request_repaint();
            return;
        }

        dialog.closing = true;
        ctx.request_repaint();
        return;
    }

    menu.screen = Screen::MainMenu;
}

fn direct_connect_dialog_ui(
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
    let id = egui::Id::new("direct_connect_modal");
    let animation = ctx.animate_bool_with_time(id.with("animation"), open, 0.16);
    if animation > 0.0 && animation < 1.0 {
        ctx.request_repaint();
    }

    if !open && animation <= 0.01 {
        return DirectConnectModalOutput {
            choice: None,
            finished_closing: true,
        };
    }

    let screen_rect = ctx.content_rect();
    let backdrop_response = egui::Area::new(id.with("backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let local_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, screen_rect.size());
            let response = ui.allocate_rect(local_rect, egui::Sense::click());
            ui.painter().rect_filled(
                local_rect,
                0,
                egui::Color32::from_rgba_unmultiplied(1, 3, 8, (190.0 * animation) as u8),
            );
            response
        })
        .inner;

    let panel_width = screen_rect.width().clamp(340.0, 440.0);
    let mut choice = None;
    let panel_response = egui::Area::new(id.with("panel"))
        .order(egui::Order::Tooltip)
        .anchor(
            egui::Align2::CENTER_CENTER,
            [0.0, 18.0 * (1.0 - animation.clamp(0.0, 1.0))],
        )
        .show(ctx, |ui| {
            ui.set_width(panel_width);
            ui.multiply_opacity(animation);
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(12, 17, 23, 246))
                .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
                .corner_radius(7)
                .inner_margin(egui::Margin::symmetric(24, 22))
                .show(ui, |ui| {
                    ui.set_width(panel_width - 48.0);
                    draw_direct_connect_form(ui, dialog, &mut choice);
                });
        })
        .response;

    let connecting = dialog.is_connecting();
    if open && choice.is_none() && !connecting && modal::confirm_shortcut_pressed(ctx) {
        choice = Some(DirectConnectChoice::Connect);
    }

    if open && choice.is_none() && !connecting && backdrop_response.clicked() {
        let clicked_outside_panel = ctx.input(|input| {
            input
                .pointer
                .interact_pos()
                .is_some_and(|position| !panel_response.rect.contains(position))
        });
        if clicked_outside_panel {
            choice = Some(DirectConnectChoice::Cancel);
        }
    }

    DirectConnectModalOutput {
        choice,
        finished_closing: false,
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

fn direct_connect_target(dialog: &DirectConnectDialog) -> Result<DirectConnectTarget> {
    let host_input = dialog.host.trim();
    if let Ok(addr) = host_input.parse::<SocketAddr>() {
        return Ok(DirectConnectTarget {
            host: addr.ip().to_string(),
            port: addr.port(),
        });
    }

    let (host, port_input) =
        split_inline_host_port(host_input).unwrap_or((host_input, dialog.port.trim()));
    if host.is_empty() {
        bail!("Server address is required.");
    }

    let Ok(port) = port_input.parse::<u16>() else {
        bail!("Port must be a number between 1 and 65535.");
    };
    if port == 0 {
        bail!("Port must be a number between 1 and 65535.");
    }

    Ok(DirectConnectTarget {
        host: host.trim_matches(['[', ']']).to_owned(),
        port,
    })
}

fn resolve_direct_connect_target(target: &DirectConnectTarget) -> Result<SocketAddr> {
    let host = target.host.trim();
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, target.port));
    }

    (host, target.port)
        .to_socket_addrs()
        .map_err(|_| anyhow::anyhow!("Could not resolve server address."))?
        .next()
        .ok_or_else(|| anyhow::anyhow!("Could not resolve server address."))
}

fn split_inline_host_port(host_input: &str) -> Option<(&str, &str)> {
    if let Some(bracketed) = host_input.strip_prefix('[') {
        let (host, port) = bracketed.rsplit_once("]:")?;
        return Some((host, port));
    }

    if host_input.matches(':').count() == 1 {
        return host_input.rsplit_once(':');
    }

    None
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

    fn dialog(host: &str, port: &str) -> DirectConnectDialog {
        DirectConnectDialog {
            host: host.to_owned(),
            port: port.to_owned(),
            error: None,
            closing: false,
            attempt: None,
        }
    }

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
    fn direct_connect_target_parses_ip_host_and_port() {
        let dialog = dialog("127.0.0.1", "7777");

        assert_eq!(
            direct_connect_target(&dialog).expect("target should parse"),
            DirectConnectTarget {
                host: "127.0.0.1".to_owned(),
                port: 7777,
            }
        );
    }

    #[test]
    fn direct_connect_target_accepts_pasted_host_and_port() {
        let dialog = dialog("127.0.0.1:8888", "7777");

        assert_eq!(
            direct_connect_target(&dialog).expect("target should parse"),
            DirectConnectTarget {
                host: "127.0.0.1".to_owned(),
                port: 8888,
            }
        );
    }

    #[test]
    fn direct_connect_target_rejects_empty_host_and_invalid_port() {
        assert!(direct_connect_target(&dialog(" ", "7777")).is_err());
        assert!(direct_connect_target(&dialog("127.0.0.1", "0")).is_err());
    }

    #[test]
    fn resolve_direct_connect_target_handles_ip_without_dns() {
        let target = DirectConnectTarget {
            host: "127.0.0.1".to_owned(),
            port: 7777,
        };

        assert_eq!(
            resolve_direct_connect_target(&target).expect("target should resolve"),
            SocketAddr::from(([127, 0, 0, 1], 7777))
        );
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
            |ctx| handle_multiplayer_escape(ctx, &mut menu),
        );

        assert_eq!(menu.screen, Screen::Multiplayer);
        let dialog = menu
            .direct_connect
            .expect("dialog should remain open while connecting");
        assert!(!dialog.closing);
    }
}
