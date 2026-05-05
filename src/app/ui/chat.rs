use bevy_egui::egui;

use crate::protocol::ClientMessage;

use crate::app::state::{ClientRuntime, MenuState};

const CHAT_WIDTH: f32 = 390.0;

pub(super) fn chat_ui(ctx: &egui::Context, menu: &mut MenuState, runtime: &mut ClientRuntime) {
    egui::Area::new("chat".into())
        .anchor(egui::Align2::LEFT_BOTTOM, [16.0, -16.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(6, 9, 13, 150))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(115, 132, 151, 58),
                ))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(10, 9))
                .show(ui, |ui| {
                    ui.set_width(CHAT_WIDTH);
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .max_height(132.0)
                        .show(ui, |ui| {
                            for message in &runtime.messages {
                                ui.label(egui::RichText::new(message).size(12.5).color(
                                    egui::Color32::from_rgba_unmultiplied(224, 231, 238, 210),
                                ));
                            }
                        });

                    ui.add_space(5.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut menu.chat_input)
                            .hint_text("Chat")
                            .desired_width(CHAT_WIDTH - 20.0)
                            .margin(egui::Margin::symmetric(8, 5)),
                    );
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        send_chat(menu, runtime);
                    }
                });
        });
}

fn send_chat(menu: &mut MenuState, runtime: &mut ClientRuntime) {
    let text = std::mem::take(&mut menu.chat_input);
    if text.trim().is_empty() {
        return;
    }

    if let Some(session) = runtime.session.as_mut()
        && let Err(error) = session.send(ClientMessage::Chat { text })
    {
        runtime.messages.push(format!("chat send failed: {error}"));
    }
}
