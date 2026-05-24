use bevy::{ecs::system::SystemParam, prelude::*};

use crate::{
    app::{
        audio::surface::SurfaceMaterial,
        state::{
            ClientErrorToast, ClientRuntime, MenuState, NoticeDialog, RemoteImpactEvent, Screen,
            SessionShutdownTasks, ToastState,
        },
        ui::ButtonSoundRequests,
        voice::IncomingVoiceMessage,
    },
    items::ToolKind,
    protocol::{ResourceImpactKind, ServerMessage, ToastKind, Vec3Net},
};

/// Fan-out writers for messages the network tick produces — voice frames,
/// remote impacts, error toasts. Grouped so the system signature stays
/// readable.
#[derive(SystemParam)]
pub(crate) struct NetworkTickWriters<'w> {
    pub(crate) remote_impacts: MessageWriter<'w, RemoteImpactEvent>,
    pub(crate) error_toasts: MessageWriter<'w, ClientErrorToast>,
    pub(crate) voice_messages: MessageWriter<'w, IncomingVoiceMessage>,
}

pub(crate) fn network_tick_system(
    time: Res<Time>,
    mut runtime: ResMut<ClientRuntime>,
    mut menu: ResMut<MenuState>,
    mut button_sound_requests: ResMut<ButtonSoundRequests>,
    mut toasts: ResMut<ToastState>,
    mut writers: NetworkTickWriters,
) {
    toasts.tick(time.delta_secs());

    if !network_tick_allowed(&menu) {
        return;
    }

    // Step the client-side day/night clock every frame the network tick
    // runs, before any new server snapshots overwrite it. The server's
    // routine `WorldTime` broadcast realigns drift; this keeps the visible
    // sun/moon smooth in between.
    runtime.tick_world_time(time.delta_secs());

    let tick_result = runtime
        .session
        .as_mut()
        .map(|session| session.tick(time.delta_secs()));
    let messages = match tick_result {
        Some(Ok(messages)) => messages,
        Some(Err(error)) => {
            let text = format!("network error: {error}");
            runtime.push_error_message(text.clone());
            writers.error_toasts.write(ClientErrorToast::new(text));
            Vec::new()
        }
        None => Vec::new(),
    };

    if messages.is_empty() {
        runtime.tick_connection_silence(time.delta_secs());
    }

    for message in messages {
        if let ServerMessage::Kicked { reason } = &message {
            let reason = reason.clone();
            runtime.apply_message(message);
            runtime.stop_session_after_kick();
            show_kick_notice(&mut menu, reason.clone());
            // Wipe in-flight gather toasts: the player is back at the main
            // menu, those messages aren't relevant anymore. The disconnect
            // notice is delivered via the modal dialog above, so no
            // replacement toast is needed.
            toasts.clear();
            let _ = reason;
            continue;
        }
        if let ServerMessage::AuthRejected { reason } = &message {
            writers
                .error_toasts
                .write(ClientErrorToast::new(format!("auth rejected: {reason}")));
        }
        if matches!(message, ServerMessage::ItemMerged { .. }) {
            button_sound_requests.push_hover();
        }
        if let ServerMessage::Toast(payload) = &message {
            toasts.push_message(payload.clone());
        }
        if let ServerMessage::ResourceImpact { position, kind } = &message {
            writers
                .remote_impacts
                .write(remote_impact_event(*position, *kind));
        }
        if let ServerMessage::Voice {
            speaker,
            sequence,
            position,
            frame,
        } = &message
        {
            writers.voice_messages.write(IncomingVoiceMessage {
                speaker: *speaker,
                sequence: *sequence,
                position: *position,
                frame: frame.clone(),
            });
        }
        runtime.apply_message(message);
    }
}

/// Reads queued [`ClientErrorToast`] events and surfaces them on
/// [`ToastState`]. Centralising the write keeps every error-emitting
/// system free of `ResMut<ToastState>` and makes the path from "an
/// error happens" to "the player sees a toast" a single hop through an
/// event channel.
pub(crate) fn surface_client_error_toasts_system(
    mut toasts: ResMut<ToastState>,
    mut events: MessageReader<ClientErrorToast>,
) {
    for event in events.read() {
        toasts.push(ToastKind::Error, event.text.clone());
    }
}

fn remote_impact_event(position: Vec3Net, kind: ResourceImpactKind) -> RemoteImpactEvent {
    let (tool, surface) = remote_impact_tool_and_surface(kind);
    RemoteImpactEvent {
        anchor: Vec3::new(position.x, position.y, position.z),
        tool,
        surface,
        effect_kind: crate::app::state::ImpactEffectKind::for_resource_impact(kind),
        // Remote impacts have no client-side swing seed; pick something
        // stable per-event so the chip burst is deterministic but varies
        // between consecutive hits.
        seed: position_seed(position),
    }
}

fn remote_impact_tool_and_surface(kind: ResourceImpactKind) -> (ToolKind, SurfaceMaterial) {
    // The server enforces tool requirements (axe → trees, pickaxe → ores,
    // hands → crude materials), so the kind uniquely determines the
    // (tool, surface) pair the swinger must have used.
    match kind {
        ResourceImpactKind::Tree => (ToolKind::Axe, SurfaceMaterial::Wood),
        ResourceImpactKind::CoalOre => (ToolKind::Pickaxe, SurfaceMaterial::Coal),
        ResourceImpactKind::IronOre => (ToolKind::Pickaxe, SurfaceMaterial::Iron),
        ResourceImpactKind::SulfurOre => (ToolKind::Pickaxe, SurfaceMaterial::Sulfur),
        ResourceImpactKind::StoneVein => (ToolKind::Pickaxe, SurfaceMaterial::Stone),
        ResourceImpactKind::Branches => (ToolKind::Hands, SurfaceMaterial::Wood),
        ResourceImpactKind::SurfaceStone => (ToolKind::Hands, SurfaceMaterial::Stone),
        ResourceImpactKind::HayGrass => (ToolKind::Hands, SurfaceMaterial::Dirt),
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
