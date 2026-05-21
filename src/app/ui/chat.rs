use std::collections::VecDeque;

use bevy_egui::egui;

use crate::{
    app::state::{ClientLogEntry, ClientLogKind, ClientRuntime, ErrorToastSink, MenuState},
    protocol::ClientMessage,
};

use super::theme;

const CHAT_WIDTH: f32 = 430.0;
const CHAT_INACTIVE_MESSAGE_HEIGHT: f32 = 122.0;
const CHAT_ACTIVE_MESSAGE_HEIGHT: f32 = 156.0;
const CHAT_INPUT_WIDTH: f32 = CHAT_WIDTH - 22.0;
const CHAT_INPUT_HEIGHT: f32 = 30.0;
const CHAT_INPUT_ID: &str = "game_chat_input";

pub(super) fn chat_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    inventory_open: bool,
) {
    let opened_this_frame = handle_chat_shortcuts(ctx, menu);
    let active = menu.chat_open && !menu.pause_open;
    let allow_pointer_interaction = active || inventory_open;

    egui::Area::new("chat".into())
        .order(egui::Order::Foreground)
        .interactable(allow_pointer_interaction)
        .anchor(egui::Align2::LEFT_BOTTOM, [18.0, -18.0])
        .show(ctx, |ui| {
            chat_frame(active).show(ui, |ui| {
                ui.set_width(CHAT_WIDTH);
                draw_messages(ui, &runtime.messages, active, allow_pointer_interaction);
                ui.add_space(8.0);
                if active {
                    draw_active_input(ui, menu, runtime, error_toasts, opened_this_frame);
                } else {
                    draw_inactive_input(ui, menu, allow_pointer_interaction);
                }
            });
        });
}

fn handle_chat_shortcuts(ctx: &egui::Context, menu: &mut MenuState) -> bool {
    if menu.pause_open || menu.chat_open {
        return false;
    }

    let should_open = ctx.input(|input| {
        let modifiers_allow_text =
            !input.modifiers.command && !input.modifiers.ctrl && !input.modifiers.alt;
        modifiers_allow_text
            && (input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::T))
    });

    if should_open {
        menu.chat_open = true;
        menu.chat_focus_pending = true;
        menu.chat_input.clear();
    }

    should_open
}

fn chat_frame(active: bool) -> egui::Frame {
    let fill_alpha = if active { 224 } else { 150 };
    let stroke_alpha = if active { 108 } else { 48 };
    egui::Frame::NONE
        .fill(egui::Color32::from_rgba_unmultiplied(5, 8, 12, fill_alpha))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(115, 132, 151, stroke_alpha),
        ))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(11, 10))
}

fn draw_messages(
    ui: &mut egui::Ui,
    messages: &VecDeque<ClientLogEntry>,
    active: bool,
    scroll_enabled: bool,
) {
    let max_height = if active {
        CHAT_ACTIVE_MESSAGE_HEIGHT
    } else {
        CHAT_INACTIVE_MESSAGE_HEIGHT
    };

    let mut scroll_area = egui::ScrollArea::vertical()
        .id_salt(if scroll_enabled {
            "chat_messages_scroll"
        } else {
            "chat_messages_static"
        })
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .max_height(max_height);

    if !scroll_enabled {
        scroll_area = scroll_area
            .scroll_source(egui::containers::scroll_area::ScrollSource::NONE)
            .scroll_bar_visibility(
                egui::containers::scroll_area::ScrollBarVisibility::AlwaysHidden,
            );
    }

    scroll_area.show(ui, |ui| {
        ui.set_width(CHAT_WIDTH - 22.0);
        for message in messages {
            draw_message(ui, message);
        }
    });
}

fn draw_message(ui: &mut egui::Ui, message: &ClientLogEntry) {
    match &message.kind {
        ClientLogKind::System => {
            ui.label(
                egui::RichText::new(&message.text)
                    .size(12.5)
                    .color(egui::Color32::from_rgb(198, 185, 143)),
            );
        }
        ClientLogKind::Error => {
            ui.label(
                egui::RichText::new(&message.text)
                    .size(12.5)
                    .color(egui::Color32::from_rgb(255, 128, 128)),
            );
        }
        ClientLogKind::Chat { from } => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    egui::RichText::new("[Global]")
                        .size(12.5)
                        .strong()
                        .color(egui::Color32::from_rgb(91, 168, 255)),
                );
                ui.label(
                    egui::RichText::new(from)
                        .size(12.5)
                        .strong()
                        .color(egui::Color32::from_rgb(230, 232, 236)),
                );
                ui.label(
                    egui::RichText::new(":")
                        .size(12.5)
                        .color(theme::muted_text()),
                );
                ui.label(
                    egui::RichText::new(&message.text)
                        .size(12.5)
                        .color(egui::Color32::from_rgb(224, 231, 238)),
                );
            });
        }
    }
}

fn draw_active_input(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    opened_this_frame: bool,
) {
    let input_id = egui::Id::new(CHAT_INPUT_ID);
    let rect = allocate_input_rect(ui);
    paint_input_background(ui, rect);
    let response = ui.put(
        input_text_rect(rect),
        theme::text_input(&mut menu.chat_input)
            .id(input_id)
            .frame(false)
            .hint_text("Chat"),
    );

    let focus_requested = opened_this_frame || menu.chat_focus_pending;
    if focus_requested {
        response.request_focus();
        menu.chat_focus_pending = false;
    }

    let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
    let escape_pressed = ui.input(|input| input.key_pressed(egui::Key::Escape));

    if escape_pressed {
        cancel_chat(menu, &response);
    } else if !focus_requested && enter_pressed && (response.has_focus() || response.lost_focus()) {
        submit_chat(menu, runtime, error_toasts, &response);
    } else if !focus_requested && response.lost_focus() {
        cancel_chat(menu, &response);
    }
}

fn draw_inactive_input(ui: &mut egui::Ui, menu: &mut MenuState, interactive: bool) {
    let rect = allocate_input_rect(ui);
    paint_input_background(ui, rect);
    ui.put(
        input_text_rect(rect),
        theme::text_input(&mut menu.chat_input)
            .id(egui::Id::new(CHAT_INPUT_ID))
            .frame(false)
            .interactive(false)
            .hint_text("Chat"),
    );

    if !interactive {
        return;
    }

    let response = ui.interact(
        rect,
        egui::Id::new(CHAT_INPUT_ID).with("inactive"),
        egui::Sense::click(),
    );
    if response.clicked() {
        menu.chat_open = true;
        menu.chat_focus_pending = true;
        menu.chat_input.clear();
    }
}

fn allocate_input_rect(ui: &mut egui::Ui) -> egui::Rect {
    ui.allocate_exact_size(
        egui::vec2(CHAT_INPUT_WIDTH, CHAT_INPUT_HEIGHT),
        egui::Sense::hover(),
    )
    .0
}

fn input_text_rect(rect: egui::Rect) -> egui::Rect {
    rect.shrink2(egui::vec2(10.0, 0.0))
}

fn paint_input_background(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().rect(
        rect,
        4,
        egui::Color32::from_rgba_unmultiplied(3, 5, 8, 156),
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(115, 132, 151, 44),
        ),
        egui::StrokeKind::Inside,
    );
}

fn submit_chat(
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    response: &egui::Response,
) {
    let text = std::mem::take(&mut menu.chat_input);
    menu.chat_open = false;
    menu.chat_focus_pending = false;
    response.surrender_focus();

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    // Leading slash routes through the server's command dispatcher rather
    // than broadcasting as chat. The server is the source of truth for
    // parsing, admin checks, and any state mutation — keeps clients honest.
    let message = if let Some(body) = trimmed.strip_prefix('/') {
        ClientMessage::Command {
            text: body.to_owned(),
        }
    } else {
        ClientMessage::Chat { text: text.clone() }
    };
    let send_label = if matches!(message, ClientMessage::Command { .. }) {
        "command send failed"
    } else {
        "chat send failed"
    };

    let failure: Option<String> = if let Some(session) = runtime.session.as_mut() {
        session
            .send(message)
            .err()
            .map(|error| format!("{send_label}: {error}"))
    } else {
        Some(format!("{send_label}: not connected"))
    };
    if let Some(text) = failure {
        runtime.push_error_message(text.clone());
        error_toasts.push_error(text);
    }
}

fn cancel_chat(menu: &mut MenuState, response: &egui::Response) {
    menu.chat_open = false;
    menu.chat_focus_pending = false;
    menu.chat_input.clear();
    response.surrender_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn key_press(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn pointer_scroll_over_chat() -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(egui::pos2(40.0, 500.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -48.0),
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    fn many_messages() -> VecDeque<ClientLogEntry> {
        (0..20)
            .map(|index| ClientLogEntry {
                kind: ClientLogKind::System,
                text: format!("message {index}"),
            })
            .collect()
    }

    #[test]
    fn shortcut_opens_chat_when_text_modifiers_are_clear() {
        let ctx = egui::Context::default();
        let mut menu = MenuState::default();

        let _ = ctx.run(
            raw_input(vec![key_press(egui::Key::T, egui::Modifiers::default())]),
            |ctx| {
                assert!(handle_chat_shortcuts(ctx, &mut menu));
            },
        );

        assert!(menu.chat_open);
        assert!(menu.chat_focus_pending);
        assert!(menu.chat_input.is_empty());
    }

    #[test]
    fn shortcut_ignores_command_modified_text_keys() {
        let ctx = egui::Context::default();
        let mut menu = MenuState::default();
        let modifiers = egui::Modifiers {
            command: true,
            ..Default::default()
        };

        let mut input = raw_input(vec![key_press(egui::Key::T, modifiers)]);
        input.modifiers = modifiers;
        let _ = ctx.run(input, |ctx| {
            assert!(!handle_chat_shortcuts(ctx, &mut menu));
        });

        assert!(!menu.chat_open);
    }

    #[test]
    fn chat_ui_renders_inactive_and_active_states() {
        let ctx = egui::Context::default();
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime {
            messages: VecDeque::from([
                ClientLogEntry {
                    kind: ClientLogKind::System,
                    text: "connected".to_owned(),
                },
                ClientLogEntry {
                    kind: ClientLogKind::Error,
                    text: "failed".to_owned(),
                },
                ClientLogEntry {
                    kind: ClientLogKind::Chat {
                        from: "Dannie".to_owned(),
                    },
                    text: "hello".to_owned(),
                },
            ]),
            ..Default::default()
        };

        let _ = ctx.run(raw_input(Vec::new()), |ctx| {
            chat_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &mut Vec::<String>::new(),
                false,
            );
        });

        menu.chat_open = true;
        menu.chat_focus_pending = true;
        let _ = ctx.run(raw_input(Vec::new()), |ctx| {
            chat_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &mut Vec::<String>::new(),
                false,
            );
        });

        assert!(!menu.chat_focus_pending);
    }

    #[test]
    fn inactive_gameplay_chat_does_not_capture_pointer_scroll() {
        let ctx = egui::Context::default();
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime {
            messages: many_messages(),
            ..Default::default()
        };
        let mut wants_pointer = true;
        let mut scroll_delta = egui::Vec2::ZERO;

        let _ = ctx.run(raw_input(Vec::new()), |ctx| {
            chat_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &mut Vec::<String>::new(),
                false,
            );
        });
        let _ = ctx.run(raw_input(pointer_scroll_over_chat()), |ctx| {
            chat_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &mut Vec::<String>::new(),
                false,
            );
            wants_pointer = ctx.wants_pointer_input();
            scroll_delta = ctx.input(|input| input.smooth_scroll_delta);
        });

        assert!(!wants_pointer);
        assert_ne!(scroll_delta, egui::Vec2::ZERO);
    }

    #[test]
    fn inventory_chat_can_capture_pointer_scroll() {
        let ctx = egui::Context::default();
        let mut menu = MenuState::default();
        let mut runtime = ClientRuntime {
            messages: many_messages(),
            ..Default::default()
        };
        let mut wants_pointer = false;

        let _ = ctx.run(raw_input(Vec::new()), |ctx| {
            chat_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &mut Vec::<String>::new(),
                true,
            );
        });
        let _ = ctx.run(raw_input(pointer_scroll_over_chat()), |ctx| {
            chat_ui(
                ctx,
                &mut menu,
                &mut runtime,
                &mut Vec::<String>::new(),
                true,
            );
            wants_pointer = ctx.wants_pointer_input();
        });

        assert!(wants_pointer);
    }

    #[test]
    fn submit_and_cancel_chat_update_menu_state() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            chat_open: true,
            chat_focus_pending: true,
            chat_input: "hello".to_owned(),
            ..Default::default()
        };
        let mut runtime = ClientRuntime::default();
        let mut sink: Vec<String> = Vec::new();

        let _ = ctx.run(raw_input(Vec::new()), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.allocate_response(egui::vec2(20.0, 20.0), egui::Sense::click());
                submit_chat(&mut menu, &mut runtime, &mut sink, &response);
            });
        });

        assert!(!menu.chat_open);
        assert_eq!(runtime.messages.len(), 1);
        assert!(runtime.messages[0].text.contains("not connected"));
        assert_eq!(sink.len(), 1, "missing toast for failed send");
        assert!(sink[0].contains("not connected"));

        menu.chat_open = true;
        menu.chat_focus_pending = true;
        menu.chat_input = "draft".to_owned();
        let _ = ctx.run(raw_input(Vec::new()), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.allocate_response(egui::vec2(20.0, 20.0), egui::Sense::click());
                cancel_chat(&mut menu, &response);
            });
        });

        assert!(!menu.chat_open);
        assert!(!menu.chat_focus_pending);
        assert!(menu.chat_input.is_empty());
    }

    #[test]
    fn input_rect_keeps_text_inside_background() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(200.0, 30.0));
        let text_rect = input_text_rect(rect);

        assert_eq!(text_rect.left(), rect.left() + 10.0);
        assert_eq!(text_rect.right(), rect.right() - 10.0);
        assert_eq!(chat_frame(true).fill.a(), 224);
        assert_eq!(chat_frame(false).fill.a(), 150);
    }
}
