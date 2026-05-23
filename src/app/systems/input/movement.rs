use bevy::{
    ecs::system::SystemParam,
    prelude::*,
    window::{PrimaryWindow, Window},
};

use crate::{
    app::state::{
        ClientErrorToast, ClientRuntime, ClientSettings, KeyAction, LookState, MenuState,
    },
    protocol::{ClientMessage, PlayerInput, PlayerMovement, Vec3Net},
};

use super::gating::{
    gameplay_accepts_controls, gameplay_simulation_allowed, primary_window_focused,
};

#[derive(SystemParam)]
pub(crate) struct ClientInputParams<'w, 's> {
    time: Res<'w, Time>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    settings: Res<'w, ClientSettings>,
    runtime: ResMut<'w, ClientRuntime>,
    menu: Res<'w, MenuState>,
    look: Res<'w, LookState>,
    error_toasts: MessageWriter<'w, ClientErrorToast>,
    primary_window: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}

pub(crate) fn client_input_system(mut params: ClientInputParams) {
    if !gameplay_simulation_allowed(&params.menu) {
        return;
    }
    if params.runtime.client_id.is_none() {
        return;
    }

    let accepts_movement_input =
        gameplay_accepts_controls(&params.menu, primary_window_focused(&params.primary_window));
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
        sprint: accepts_movement_input
            && params
                .settings
                .keybindings
                .pressed(KeyAction::Sprint, &params.keys),
        jump: accepts_movement_input
            && params
                .settings
                .keybindings
                .just_pressed(KeyAction::Jump, &params.keys),
        yaw: params.look.yaw,
        pitch: params.look.pitch,
    };

    // Split-borrow: `world_grid` is read-only here while `predicted_local`
    // is mutated. Reborrowing through `&mut *runtime` lets the compiler see
    // the two fields as disjoint, avoiding a per-frame `BlockGrid` rebuild.
    let runtime = &mut *params.runtime;
    let mut movement = None;
    if let (Some(predicted), Some(grid)) = (
        runtime.predicted_local.as_mut(),
        runtime.world_grid.as_ref(),
    ) {
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
        let send_result = runtime
            .session
            .as_mut()
            .map(|session| session.send(ClientMessage::Movement(movement)));
        if let Some(Err(error)) = send_result {
            let text = format!("movement send failed: {error}");
            runtime.push_error_message(text.clone());
            params.error_toasts.write(ClientErrorToast::new(text));
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
