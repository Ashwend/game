use bevy::{ecs::system::SystemParam, prelude::*};

use crate::{
    analytics::SessionEndReason,
    app::{
        audio::surface::SurfaceMaterial,
        state::{
            ClientErrorToast, ClientRuntime, MenuState, NoticeDialog, RemoteImpactEvent, SaveStore,
            Screen, SessionShutdownTasks, ToastState,
        },
        systems::PendingSessionEndReason,
        ui::{
            ButtonSoundRequests,
            floating_text::{FloatingDamageRole, FloatingDamageText},
        },
        voice::IncomingVoiceMessage,
    },
    items::ToolKind,
    protocol::{
        ClientMessage, GAME_VERSION, ResourceImpactKind, ServerMessage, ToastKind, Vec3Net,
    },
};

/// How often the client sends an RTT probe (`Ping`). One per second matches the
/// server's roster broadcast cadence, so the pause-screen ping is never more
/// than a second stale.
const PING_INTERVAL_SECONDS: f32 = 1.0;

/// Fan-out writers for messages the network tick produces, voice frames,
/// remote impacts, error toasts. Grouped so the system signature stays
/// readable.
#[derive(SystemParam)]
pub(crate) struct NetworkTickWriters<'w> {
    pub(crate) remote_impacts: MessageWriter<'w, RemoteImpactEvent>,
    pub(crate) error_toasts: MessageWriter<'w, ClientErrorToast>,
    pub(crate) voice_messages: MessageWriter<'w, IncomingVoiceMessage>,
    /// PvP "I got hit" camera reaction. Fired when a `PlayerImpact`
    /// arrives whose `target` matches the local client.
    pub(crate) camera_kick: ResMut<'w, crate::app::systems::CameraImpactKick>,
    /// Transient hit marker + damage-direction state. The target side of a
    /// `PlayerImpact` pushes a directional arrow toward the attacker here.
    pub(crate) combat_feedback: ResMut<'w, crate::app::state::CombatFeedbackState>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn network_tick_system(
    time: Res<Time>,
    mut commands: Commands,
    mut runtime: ResMut<ClientRuntime>,
    mut menu: ResMut<MenuState>,
    mut button_sound_requests: ResMut<ButtonSoundRequests>,
    mut toasts: ResMut<ToastState>,
    mut writers: NetworkTickWriters,
    mut pending_session_end: ResMut<PendingSessionEndReason>,
    store: Res<SaveStore>,
    mut shutdown_tasks: ResMut<SessionShutdownTasks>,
    mut ping_accumulator: Local<f32>,
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

    // Periodic RTT probe: stamp the client clock and report the last measured
    // latency; the server echoes the timestamp back in `Pong` and stores the
    // reported value for the roster broadcast.
    *ping_accumulator += time.delta_secs();
    if *ping_accumulator >= PING_INTERVAL_SECONDS {
        *ping_accumulator = 0.0;
        let client_time_ms = (time.elapsed_secs() * 1000.0) as u32;
        let rtt_ms = runtime.local_ping_ms;
        if let Some(session) = runtime.session.as_mut() {
            let _ = session.send(ClientMessage::Ping {
                client_time_ms,
                rtt_ms,
            });
        }
    }

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
            pending_session_end.0 = Some(SessionEndReason::Kick);
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
            let reason = reason.clone();
            // The server refused us at the handshake (bad/expired auth
            // ticket, protocol or version mismatch). Bucket it as a
            // disconnect for analytics, log it, gracefully tear the session
            // down, and bounce back to the main menu with a friendly notice.
            pending_session_end.0 = Some(SessionEndReason::Disconnect);
            runtime.apply_message(message);
            // Graceful, non-blocking teardown: the DISCONNECT handshake runs
            // on a worker thread (`shutdown_in_background`) rather than
            // freezing the main thread on an inline session drop.
            runtime.shutdown_in_background(store.0.clone(), &mut shutdown_tasks);
            show_auth_rejected_notice(&mut menu, reason);
            // In-flight gather toasts aren't relevant back at the menu; the
            // notice modal carries the message.
            toasts.clear();
            continue;
        }
        if let ServerMessage::VersionMismatch { server_version, .. } = &message {
            let server_version = server_version.clone();
            // Client/server build mismatch. Same graceful teardown as an auth
            // rejection, but the modal shows both versions so the player can
            // see whether they're ahead of or behind the server.
            pending_session_end.0 = Some(SessionEndReason::Disconnect);
            runtime.apply_message(message);
            runtime.shutdown_in_background(store.0.clone(), &mut shutdown_tasks);
            show_version_mismatch_notice(&mut menu, server_version);
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
            writers
                .remote_impacts
                .write(remote_impact_event(*position, *kind));
        }
        if let ServerMessage::PlayerKilled { killer_name, .. } = &message {
            menu.death_splash = Some(crate::app::state::DeathSplash::new(killer_name.clone()));
            // Pop any pause/inventory overlays so the splash is the
            // only modal, the player can't pause-out of the death
            // screen.
            menu.pause_open = false;
            menu.inventory_open = false;
            menu.crafting_open = false;
            menu.furnace_open = false;
            menu.chat_open = false;
        }
        // The server replies to Respawn with a `Correction` carrying
        // full health, so the message itself is the reliable "the
        // player just respawned" signal, more robust than watching
        // the replicated `PlayerLifecycle` flip, which can be missed
        // if the player's mirror entity crosses a chunk-room boundary
        // at the same tick. Instead of clearing the splash outright
        // we kick off its close-fade so the new HUD doesn't pop in
        // under a still-black screen for a frame.
        if let ServerMessage::Correction(state) = &message
            && runtime.client_id == Some(state.client_id)
            && state.health > 0.0
            && let Some(splash) = menu.death_splash.as_mut()
        {
            splash.begin_closing();
        }
        if let ServerMessage::PlayerImpact {
            attacker,
            target,
            position,
            attacker_position,
            tool,
            damage_dealt,
        } = &message
        {
            // Reuse the `RemoteImpactEvent` channel as resource hits
            // so peers see a chip burst at the target's chest.
            // `is_player_hit = true` routes the audio dispatcher to
            // the dedicated PvP impact pool; the `surface` field is
            // still set for the visual fallback only.
            writers
                .remote_impacts
                .write(crate::app::state::RemoteImpactEvent {
                    anchor: Vec3::new(position.x, position.y, position.z),
                    tool: *tool,
                    surface: SurfaceMaterial::Wood,
                    effect_kind: crate::app::state::ImpactEffectKind::FleshHit,
                    seed: position_seed(*position),
                    is_player_hit: true,
                });
            // Per-role camera + floating-text feedback. `PlayerImpact`
            // is broadcast to every peer except the attacker, so:
            //   - If the local client is the target, they get the
            //     "I just got hit" camera kick + a red number.
            //   - If the local client is a third-party observer, they
            //     see the chip burst + audio but no camera kick (it'd
            //     read as someone hitting *them*).
            //   - The attacker never receives this message; their
            //     local prediction already spawned an orange number
            //     in `dispatch_player_swing`.
            let local = runtime.client_id;
            if local == Some(*target) {
                writers.camera_kick.trigger_from_hit(*tool);
                // Point a fading direction arrow at the attacker so the
                // target can tell where the hit came from, even from
                // off-screen or behind.
                writers.combat_feedback.push_damage_from(Vec3::new(
                    attacker_position.x,
                    attacker_position.y,
                    attacker_position.z,
                ));
                commands.spawn(FloatingDamageText::new(
                    Vec3::new(position.x, position.y, position.z),
                    *damage_dealt,
                    FloatingDamageRole::Taken,
                ));
            } else if local == Some(*attacker) {
                // Reconciliation: the attacker already spawned an
                // orange number from prediction. If the server's
                // damage disagrees, we don't bother chasing the
                // delta, the chip burst from PlayerImpact never
                // arrives on the attacker side anyway, so any
                // mismatch is silent. Future hook: spawn a
                // corrective number here if the predicted value
                // doesn't match `damage_dealt`.
            } else {
                // Third-party observer: only floating text, no
                // camera kick. Use the dealt (orange) variant so a
                // bystander reads the hit as "someone scored".
                commands.spawn(FloatingDamageText::new(
                    Vec3::new(position.x, position.y, position.z),
                    *damage_dealt,
                    FloatingDamageRole::Dealt,
                ));
            }
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
        if let ServerMessage::Pong { client_time_ms } = &message {
            let now_ms = (time.elapsed_secs() * 1000.0) as u32;
            let rtt = now_ms.saturating_sub(*client_time_ms).min(u16::MAX as u32) as u16;
            runtime.set_local_ping(rtt);
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
        is_player_hit: false,
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

/// Bounces the client back to the main menu and surfaces `notice`. Used when
/// the server refuses a join (auth rejection or version mismatch). Unlike a
/// mid-game kick, this fires while the join is still in progress, so it also
/// clears the "Joining server" loading splash and any half-open connect dialog
/// that would otherwise linger over the menu.
fn return_to_menu_with_notice(menu: &mut MenuState, notice: NoticeDialog) {
    menu.notice = Some(notice);
    menu.screen = Screen::MainMenu;
    menu.loading_splash = None;
    menu.direct_connect = None;
    menu.pause_open = false;
    menu.pause_options_open = false;
    menu.inventory_open = false;
    menu.chat_open = false;
    menu.chat_focus_pending = false;
}

fn show_auth_rejected_notice(menu: &mut MenuState, reason: String) {
    return_to_menu_with_notice(menu, NoticeDialog::auth_rejected(reason));
}

fn show_version_mismatch_notice(menu: &mut MenuState, server_version: String) {
    return_to_menu_with_notice(
        menu,
        NoticeDialog::version_mismatch(GAME_VERSION, &server_version),
    );
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

    #[test]
    fn auth_rejected_notice_returns_to_main_menu_and_clears_join_overlay() {
        // Default seeds a loading splash; the join bounce must clear it so
        // the "Joining server" overlay doesn't linger over the menu.
        let mut menu = MenuState {
            screen: Screen::InGame,
            inventory_open: true,
            chat_open: true,
            chat_focus_pending: true,
            ..Default::default()
        };
        assert!(menu.loading_splash.is_some());

        show_auth_rejected_notice(&mut menu, "bad token".to_owned());

        assert_eq!(menu.screen, Screen::MainMenu);
        assert!(menu.loading_splash.is_none());
        assert!(menu.direct_connect.is_none());
        assert!(!menu.inventory_open);
        assert!(!menu.chat_open);
        assert!(!menu.chat_focus_pending);
        let notice = menu.notice.as_ref().expect("auth-rejected notice set");
        assert_eq!(notice.title, "Couldn't join server");
        assert!(
            notice.body.contains("bad token"),
            "the server reason should be surfaced: {}",
            notice.body
        );
    }

    #[test]
    fn version_mismatch_notice_returns_to_main_menu_with_both_versions() {
        let mut menu = MenuState {
            screen: Screen::InGame,
            inventory_open: true,
            ..Default::default()
        };

        show_version_mismatch_notice(&mut menu, "9.9.9".to_owned());

        assert_eq!(menu.screen, Screen::MainMenu);
        assert!(menu.loading_splash.is_none());
        assert!(!menu.inventory_open);
        let notice = menu.notice.as_ref().expect("version-mismatch notice set");
        assert_eq!(notice.title, "Version mismatch");
        assert!(notice.body.contains("9.9.9"), "shows the server version");
        assert!(
            notice.body.contains(GAME_VERSION),
            "shows the client's own version"
        );
    }

    #[test]
    fn remote_impact_tool_and_surface_maps_each_kind() {
        use crate::app::audio::surface::SurfaceMaterial;
        // Trees -> axe/wood, ores -> pickaxe with their own surface, crude
        // kinds -> hands.
        assert_eq!(
            remote_impact_tool_and_surface(ResourceImpactKind::Tree),
            (ToolKind::Axe, SurfaceMaterial::Wood)
        );
        assert_eq!(
            remote_impact_tool_and_surface(ResourceImpactKind::CoalOre),
            (ToolKind::Pickaxe, SurfaceMaterial::Coal)
        );
        assert_eq!(
            remote_impact_tool_and_surface(ResourceImpactKind::IronOre),
            (ToolKind::Pickaxe, SurfaceMaterial::Iron)
        );
        assert_eq!(
            remote_impact_tool_and_surface(ResourceImpactKind::SulfurOre),
            (ToolKind::Pickaxe, SurfaceMaterial::Sulfur)
        );
        assert_eq!(
            remote_impact_tool_and_surface(ResourceImpactKind::StoneVein),
            (ToolKind::Pickaxe, SurfaceMaterial::Stone)
        );
        assert_eq!(
            remote_impact_tool_and_surface(ResourceImpactKind::Branches),
            (ToolKind::Hands, SurfaceMaterial::Wood)
        );
        assert_eq!(
            remote_impact_tool_and_surface(ResourceImpactKind::SurfaceStone),
            (ToolKind::Hands, SurfaceMaterial::Stone)
        );
        assert_eq!(
            remote_impact_tool_and_surface(ResourceImpactKind::HayGrass),
            (ToolKind::Hands, SurfaceMaterial::Dirt)
        );
    }

    #[test]
    fn remote_impact_event_carries_position_and_resolved_pair() {
        let position = Vec3Net::new(1.0, 2.0, 3.0);
        let event = remote_impact_event(position, ResourceImpactKind::Tree);
        assert_eq!(event.anchor, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(event.tool, ToolKind::Axe);
        assert!(!event.is_player_hit);
        // The seed is derived from position and is stable.
        assert_eq!(event.seed, position_seed(position));
    }

    #[test]
    fn position_seed_is_stable_and_varies_with_position() {
        let a = Vec3Net::new(1.0, 2.0, 3.0);
        let b = Vec3Net::new(1.0, 2.0, 3.0);
        let c = Vec3Net::new(1.0, 2.0, 4.0);
        // Same position -> same seed.
        assert_eq!(position_seed(a), position_seed(b));
        // Different position -> (almost certainly) different seed.
        assert_ne!(position_seed(a), position_seed(c));
    }

    #[test]
    fn network_tick_allowed_only_in_game() {
        assert!(network_tick_allowed(&MenuState {
            screen: Screen::InGame,
            ..Default::default()
        }));
        assert!(!network_tick_allowed(&MenuState {
            screen: Screen::MainMenu,
            ..Default::default()
        }));
    }
}
