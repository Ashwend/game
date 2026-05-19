use bevy::prelude::*;

use crate::{
    app::{
        state::{
            ClientRuntime, ImpactEffectKind, MenuState, NoticeDialog, RemoteImpactEvent, Screen,
            SessionShutdownTasks, ToastState,
        },
        ui::ButtonSoundRequests,
    },
    protocol::{ResourceImpactKind, ServerMessage, Vec3Net},
};

pub(crate) fn network_tick_system(
    time: Res<Time>,
    mut runtime: ResMut<ClientRuntime>,
    mut menu: ResMut<MenuState>,
    mut button_sound_requests: ResMut<ButtonSoundRequests>,
    mut toasts: ResMut<ToastState>,
    mut remote_impacts: MessageWriter<RemoteImpactEvent>,
) {
    toasts.tick(time.delta_secs());

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
            toasts.clear();
            continue;
        }
        if matches!(message, ServerMessage::ItemMerged { .. }) {
            button_sound_requests.push_hover();
        }
        if let ServerMessage::Toast(payload) = &message {
            toasts.push_message(payload.clone());
        }
        if let ServerMessage::ResourceImpact { position, kind } = &message {
            remote_impacts.write(remote_impact_event(*position, *kind));
        }
        runtime.apply_message(message);
    }
}

fn remote_impact_event(position: Vec3Net, kind: ResourceImpactKind) -> RemoteImpactEvent {
    RemoteImpactEvent {
        anchor: Vec3::new(position.x, position.y, position.z),
        kind: match kind {
            ResourceImpactKind::Tree => ImpactEffectKind::WoodChips,
            ResourceImpactKind::OreNode => ImpactEffectKind::StoneShards,
        },
        // Remote impacts have no client-side swing seed; pick something
        // stable per-event so the chip burst is deterministic but varies
        // between consecutive hits.
        seed: position_seed(position),
    }
}

fn position_seed(position: Vec3Net) -> u32 {
    let x = position.x.to_bits();
    let y = position.y.to_bits();
    let z = position.z.to_bits();
    x.wrapping_mul(0x9E3779B1)
        .wrapping_add(y.wrapping_mul(0x85EBCA77))
        .wrapping_add(z.wrapping_mul(0xC2B2AE3D))
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
