use bevy::prelude::*;

use crate::{
    app::{
        state::{ClientRuntime, MenuState, Screen, SessionShutdownTasks},
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
    if !network_tick_allowed(&menu) {
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

fn network_tick_allowed(menu: &MenuState) -> bool {
    menu.screen == Screen::InGame
}

pub(crate) fn session_shutdown_poll_system(
    mut menu: ResMut<MenuState>,
    mut shutdown_tasks: ResMut<SessionShutdownTasks>,
) {
    for result in shutdown_tasks.drain_finished() {
        if let Err(error) = result {
            menu.status = Some(format!("save/shutdown error: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_menu_does_not_block_network_ticks() {
        let paused = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            ..Default::default()
        };
        assert!(network_tick_allowed(&paused));

        let main_menu = MenuState {
            screen: Screen::MainMenu,
            ..Default::default()
        };
        assert!(!network_tick_allowed(&main_menu));
    }
}
