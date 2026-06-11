//! In-game single-field text dialog: door lock codes (set / enter /
//! change) and sleeping-bag rename. One prompt slot on `MenuState`; the
//! prompt gates gameplay controls like every other overlay and submits
//! straight to the server message that owns the action.

use bevy_egui::egui;

use crate::{
    app::state::{ClientRuntime, ErrorToastSink, MenuState, TextPrompt, TextPromptKind},
    game_balance::{DOOR_CODE_MAX_LEN, DOOR_CODE_MIN_LEN, SLEEPING_BAG_NAME_MAX_LEN},
    protocol::{ClientMessage, DoorCommand, SleepingBagCommand},
};

use super::{
    modal::{self},
    theme,
};

/// Copy + validation per prompt kind.
struct PromptSpec {
    title: &'static str,
    body: &'static str,
    confirm_label: &'static str,
    numeric: bool,
    char_limit: usize,
}

fn spec(kind: &TextPromptKind) -> PromptSpec {
    match kind {
        TextPromptKind::DoorSetCode { .. } => PromptSpec {
            title: "Set Door Code",
            body: "Pick a 4-6 digit code for this door's lock. You'll enter \
                   it yourself once to unlock the door.",
            confirm_label: "Hang Door",
            numeric: true,
            char_limit: DOOR_CODE_MAX_LEN,
        },
        TextPromptKind::DoorEnterCode { .. } => PromptSpec {
            title: "Door Code",
            body: "This door is code-locked. Enter the code to unlock it, \
                   then open it with the interact key.",
            confirm_label: "Unlock",
            numeric: true,
            char_limit: DOOR_CODE_MAX_LEN,
        },
        TextPromptKind::DoorChangeCode { .. } => PromptSpec {
            title: "Change Door Code",
            body: "Set a new 4-6 digit code. Everyone else loses access \
                   until they enter the new one.",
            confirm_label: "Change Code",
            numeric: true,
            char_limit: DOOR_CODE_MAX_LEN,
        },
        TextPromptKind::RenameBag { .. } => PromptSpec {
            title: "Rename Sleeping Bag",
            body: "Name this bag so you can tell your respawn points apart. \
                   Leave it empty to clear the name.",
            confirm_label: "Rename",
            numeric: false,
            char_limit: SLEEPING_BAG_NAME_MAX_LEN,
        },
    }
}

fn input_is_valid(prompt: &TextPrompt) -> bool {
    match prompt.kind {
        TextPromptKind::RenameBag { .. } => true,
        _ => {
            (DOOR_CODE_MIN_LEN..=DOOR_CODE_MAX_LEN).contains(&prompt.input.len())
                && prompt.input.bytes().all(|byte| byte.is_ascii_digit())
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptChoice {
    Confirm,
    Cancel,
}

pub(super) fn text_prompt_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
) {
    let Some(prompt) = menu.text_prompt.as_mut() else {
        return;
    };
    let prompt_spec = spec(&prompt.kind);

    let mut autofocus = prompt.autofocus_pending;
    let mut input = prompt.input.clone();
    let valid = input_is_valid(prompt);

    let output = modal::modal_shell::<PromptChoice>(
        ctx,
        "in_game_text_prompt",
        true,
        320.0,
        380.0,
        |ui, choice| {
            ui.label(theme::section(prompt_spec.title));
            ui.add_space(6.0);
            ui.label(prompt_spec.body);
            ui.add_space(10.0);

            let response = ui.add_sized(
                [ui.available_width(), 26.0],
                theme::text_input(&mut input)
                    .id(egui::Id::new("in_game_text_prompt_input"))
                    .char_limit(prompt_spec.char_limit)
                    .hint_text(if prompt_spec.numeric { "0000" } else { "Name" }),
            );
            if prompt_spec.numeric {
                input.retain(|c| c.is_ascii_digit());
            }
            if autofocus {
                response.request_focus();
                autofocus = false;
            }
            ui.add_space(12.0);

            ui.horizontal(|ui| {
                let confirm = theme::compact_button_with_state(
                    ui,
                    prompt_spec.confirm_label,
                    theme::ButtonKind::Primary,
                    130.0,
                    theme::ButtonState::Ready,
                );
                if confirm.clicked() && valid {
                    *choice = Some(PromptChoice::Confirm);
                }
                if theme::compact_button(ui, "Cancel", theme::ButtonKind::Secondary, 90.0).clicked()
                {
                    *choice = Some(PromptChoice::Cancel);
                }
            });

            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if escape {
                *choice = Some(PromptChoice::Cancel);
            }
        },
    );

    prompt.autofocus_pending = autofocus;
    prompt.input = input;

    let mut choice = output.choice;
    if choice.is_none() && output.confirm_shortcut_pressed && valid {
        choice = Some(PromptChoice::Confirm);
    }
    if choice.is_none() && output.clicked_outside {
        choice = Some(PromptChoice::Cancel);
    }
    let Some(choice) = choice else {
        return;
    };

    let Some(prompt) = menu.text_prompt.take() else {
        return;
    };
    if choice == PromptChoice::Cancel {
        return;
    }
    let message = match prompt.kind {
        TextPromptKind::DoorSetCode { doorway_id, flip } => {
            ClientMessage::Door(DoorCommand::Place {
                doorway_id,
                flip,
                code: prompt.input,
            })
        }
        TextPromptKind::DoorEnterCode { door_id } => ClientMessage::Door(DoorCommand::EnterCode {
            id: door_id,
            code: prompt.input,
        }),
        TextPromptKind::DoorChangeCode { door_id } => {
            ClientMessage::Door(DoorCommand::ChangeCode {
                id: door_id,
                code: prompt.input,
            })
        }
        TextPromptKind::RenameBag { bag_id } => {
            ClientMessage::SleepingBag(SleepingBagCommand::Rename {
                id: bag_id,
                name: prompt.input,
            })
        }
    };
    let Some(session) = runtime.session.as_mut() else {
        return;
    };
    if let Err(error) = session.send(message) {
        error_toasts.push_error(format!("couldn't send: {error}"));
    }
}
