use bevy::prelude::*;

use crate::{
    app::{
        state::{ClientRuntime, MenuState, NoticeDialog, Screen, SessionShutdownTasks},
        ui::ButtonSoundRequests,
    },
    protocol::ServerMessage,
};

pub(crate) fn network_tick_system(
    time: Res<Time>,
    mut runtime: ResMut<ClientRuntime>,
    mut menu: ResMut<MenuState>,
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
        if let ServerMessage::Kicked { reason } = &message {
            runtime.apply_message(message.clone());
            runtime.stop_session_after_kick();
            show_kick_notice(&mut menu, reason.clone());
            continue;
        }
        if matches!(message, ServerMessage::ItemMerged { .. }) {
            button_sound_requests.push_hover();
        }
        runtime.apply_message(message);
    }
}

fn show_kick_notice(menu: &mut MenuState, reason: String) {
    menu.notice = Some(NoticeDialog::disconnected(reason));
    menu.screen = Screen::MainMenu;
    menu.pause_open = false;
    menu.pause_options_open = false;
    menu.inventory_open = false;
    menu.chat_open = false;
    menu.chat_focus_pending = false;
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

    #[test]
    fn kick_notice_returns_to_main_menu() {
        let mut menu = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            inventory_open: true,
            chat_open: true,
            chat_focus_pending: true,
            ..Default::default()
        };

        show_kick_notice(&mut menu, "Server restart".to_owned());

        assert_eq!(menu.screen, Screen::MainMenu);
        assert!(!menu.pause_open);
        assert!(!menu.inventory_open);
        assert!(!menu.chat_open);
        assert!(matches!(
            menu.notice.as_ref().map(|notice| notice.body.as_str()),
            Some("Server restart")
        ));
    }
}
