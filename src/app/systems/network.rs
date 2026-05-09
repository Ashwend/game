use bevy::prelude::*;

use crate::{
    app::{
        state::{ClientRuntime, MenuState, Screen},
        ui::ButtonSoundRequests,
    },
    protocol::ServerMessage,
};

pub(crate) fn network_tick_system(
    time: Res<Time>,
    mut runtime: ResMut<ClientRuntime>,
    menu: Res<MenuState>,
    mut button_sound_requests: ResMut<ButtonSoundRequests>,
) {
    if menu.screen != Screen::InGame {
        return;
    }

    let tick_result = runtime
        .session
        .as_mut()
        .map(|session| session.tick(time.delta_secs()));
    let messages = match tick_result {
        Some(Ok(messages)) => messages,
        Some(Err(error)) => {
            runtime.push_error_message(format!("network error: {error}"));
            Vec::new()
        }
        None => Vec::new(),
    };

    for message in messages {
        if matches!(message, ServerMessage::ItemMerged { .. }) {
            button_sound_requests.push_hover();
        }
        runtime.apply_message(message);
    }
}
