use bevy::{
    ecs::system::SystemParam,
    prelude::*,
    window::{PrimaryWindow, Window},
};

use crate::{
    app::state::{
        ClientErrorToast, ClientRuntime, ClientSettings, KeyAction, LookState, MenuState,
    },
    protocol::{ClientMessage, PlayerInput, PlayerMovement, SERVER_TICK_RATE_HZ, Vec3Net},
};

use super::gating::{
    gameplay_accepts_movement, gameplay_simulation_allowed, primary_window_focused,
};

/// Outgoing movement messages per second while the player's state is
/// changing. The server simulates at [`SERVER_TICK_RATE_HZ`] and keeps
/// only the newest movement per tick, so anything past ~1.5x the tick
/// rate is discarded on arrival. Before this throttle the send rate was
/// coupled to the render frame rate: a 144 fps client sent 144
/// messages a second with ~85% of them overwritten unread. 1.5x keeps
/// every server tick fed with fresh state despite send/tick phase
/// drift.
const MOVEMENT_SEND_RATE_HZ: f32 = SERVER_TICK_RATE_HZ * 1.5;
/// Keep-alive cadence while the player is fully stationary (identical
/// position, velocity, look, and grounded state). Liveness rides the
/// separate 1 Hz heartbeat; this only bounds how stale the server's
/// view of an idle player can get.
const MOVEMENT_IDLE_SEND_RATE_HZ: f32 = 1.0;

/// Throttle state for outgoing [`ClientMessage::Movement`]. Local
/// prediction still runs every frame; only the *send* is paced.
pub(crate) struct MovementSendState {
    /// Seconds since the last movement message went out.
    since_last_send: f32,
    /// The movement most recently sent, for idle detection. The
    /// sequence field is ignored in comparisons (it advances every
    /// frame regardless).
    last_sent: Option<PlayerMovement>,
}

impl Default for MovementSendState {
    fn default() -> Self {
        Self {
            // Saturated so the first computed movement of a session
            // goes out immediately instead of waiting one interval.
            since_last_send: f32::MAX,
            last_sent: None,
        }
    }
}

/// `true` when the simulated state differs from the last sent one in
/// any field a peer or the server could observe. Sequence is excluded:
/// it increments every frame even when nothing moves.
fn movement_state_changed(last: &PlayerMovement, current: &PlayerMovement) -> bool {
    last.position != current.position
        || last.velocity != current.velocity
        || last.yaw != current.yaw
        || last.pitch != current.pitch
        || last.grounded != current.grounded
}

#[derive(SystemParam)]
pub(crate) struct ClientInputParams<'w, 's> {
    time: Res<'w, Time>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    settings: Res<'w, ClientSettings>,
    runtime: ResMut<'w, ClientRuntime>,
    menu: Res<'w, MenuState>,
    look: Res<'w, LookState>,
    local_player: Res<'w, crate::app::state::LocalPlayerState>,
    error_toasts: MessageWriter<'w, ClientErrorToast>,
    primary_window: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}

pub(crate) fn client_input_system(
    mut params: ClientInputParams,
    mut send_state: Local<MovementSendState>,
) {
    if !gameplay_simulation_allowed(&params.menu) {
        return;
    }
    if params.runtime.client_id.is_none() {
        return;
    }

    let accepts_movement_input =
        gameplay_accepts_movement(&params.menu, primary_window_focused(&params.primary_window));
    let direction = movement_direction_from_keys(
        &params.keys,
        &params.settings.keybindings,
        accepts_movement_input,
    );

    params.runtime.input_sequence += 1;
    let sequence = params.runtime.input_sequence;
    let delta_seconds = params.time.delta_secs();
    let input = PlayerInput {
        sequence,
        direction,
        run: accepts_movement_input
            && params
                .settings
                .keybindings
                .pressed(KeyAction::Run, &params.keys),
        jump: accepts_movement_input
            && params
                .settings
                .keybindings
                .just_pressed(KeyAction::Jump, &params.keys),
        yaw: params.look.yaw,
        pitch: params.look.pitch,
    };

    // Admin `/speed` cheat: the replicated multiplier (1.0 normally) is
    // re-applied every frame so a server correction that rebuilds the
    // predicted controller can't strand it at the default.
    let run_speed_multiplier = params
        .local_player
        .private
        .as_ref()
        .map(|private| private.run_speed_multiplier)
        .unwrap_or(1.0);

    // Split-borrow: `world_grid` is read-only here while `predicted_local`
    // is mutated. Reborrowing through `&mut *runtime` lets the compiler see
    // the two fields as disjoint, avoiding a per-frame `BlockGrid` rebuild.
    let runtime = &mut *params.runtime;
    let mut movement = None;
    if let (Some(predicted), Some(grid)) = (
        runtime.predicted_local.as_mut(),
        runtime.world_grid.as_ref(),
    ) {
        predicted.speed_multiplier = run_speed_multiplier;
        predicted.apply_input(input);
        predicted.simulate_with_grid(delta_seconds, grid);
        movement = Some(PlayerMovement {
            sequence,
            position: predicted.position,
            velocity: predicted.velocity,
            yaw: predicted.yaw,
            pitch: predicted.pitch,
            grounded: predicted.grounded,
        });
    }

    if let Some(movement) = movement {
        // Prediction ran above at frame rate; the wire send is paced.
        // Latest-state-wins semantics on the server (the sequence
        // guard) mean skipped frames lose nothing, the next send
        // carries the integrated result.
        send_state.since_last_send = (send_state.since_last_send + delta_seconds).min(f32::MAX);
        let changed = send_state
            .last_sent
            .is_none_or(|last| movement_state_changed(&last, &movement));
        let min_interval = if changed {
            1.0 / MOVEMENT_SEND_RATE_HZ
        } else {
            1.0 / MOVEMENT_IDLE_SEND_RATE_HZ
        };
        if send_state.since_last_send < min_interval {
            return;
        }
        send_state.since_last_send = 0.0;
        let send_result = runtime
            .session
            .as_mut()
            .map(|session| session.send(ClientMessage::Movement(movement)));
        match send_result {
            Some(Ok(())) => {
                send_state.last_sent = Some(movement);
            }
            Some(Err(error)) => {
                // Leave `last_sent` stale so the next interval retries
                // as a changed send.
                let text = format!("movement send failed: {error}");
                runtime.push_error_message(text.clone());
                params.error_toasts.write(ClientErrorToast::new(text));
            }
            None => {}
        }
    }
}

fn movement_direction_from_keys(
    keys: &ButtonInput<KeyCode>,
    bindings: &crate::app::state::KeyBindings,
    accepts_movement_input: bool,
) -> Vec3Net {
    if !accepts_movement_input {
        return Vec3Net::ZERO;
    }

    let mut direction = Vec3Net::ZERO;
    if bindings.pressed(KeyAction::MoveForward, keys) {
        direction.z += 1.0;
    }
    if bindings.pressed(KeyAction::MoveBackward, keys) {
        direction.z -= 1.0;
    }
    if bindings.pressed(KeyAction::StrafeLeft, keys) {
        direction.x -= 1.0;
    }
    if bindings.pressed(KeyAction::StrafeRight, keys) {
        direction.x += 1.0;
    }
    direction
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::KeyBindings;

    #[test]
    fn inventory_open_ignores_directional_movement_input() {
        let mut keys = ButtonInput::default();
        keys.press(KeyCode::KeyW);
        keys.press(KeyCode::KeyD);
        let bindings = KeyBindings::default();

        assert_eq!(
            movement_direction_from_keys(&keys, &bindings, true),
            Vec3Net::new(1.0, 0.0, 1.0)
        );
        assert_eq!(
            movement_direction_from_keys(&keys, &bindings, false),
            Vec3Net::ZERO
        );
    }
}
