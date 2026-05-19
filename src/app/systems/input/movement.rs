use bevy::{
    prelude::*,
    window::{PrimaryWindow, Window},
};

use crate::{
    app::state::{ClientRuntime, LookState, MenuState},
    protocol::{ClientMessage, PlayerInput, PlayerMovement, Vec3Net},
};

use super::gating::{
    gameplay_accepts_controls, gameplay_simulation_allowed, primary_window_focused,
};

pub(crate) fn client_input_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut runtime: ResMut<ClientRuntime>,
    menu: Res<MenuState>,
    look: Res<LookState>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    if !gameplay_simulation_allowed(&menu) {
        return;
    }
    if runtime.client_id.is_none() {
        return;
    }

    let accepts_movement_input =
        gameplay_accepts_controls(&menu, primary_window_focused(&primary_window));
    let direction = movement_direction_from_keys(&keys, accepts_movement_input);

    runtime.input_sequence += 1;
    let sequence = runtime.input_sequence;
    let delta_seconds = time.delta_secs();
    let input = PlayerInput {
        sequence,
        delta_seconds,
        direction,
        sprint: accepts_movement_input
            && (keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight)),
        jump: accepts_movement_input && keys.just_pressed(KeyCode::Space),
        yaw: look.yaw,
        pitch: look.pitch,
    };

    // Split-borrow: `runtime.world`/`world_grid` are read-only here while
    // `predicted_local` is mutated. Reborrowing through `&mut *runtime` lets
    // the compiler see the three fields as disjoint, avoiding a per-frame
    // `WorldData` clone and a per-frame `BlockGrid` rebuild.
    let runtime = &mut *runtime;
    let mut movement = None;
    if let (Some(predicted), Some(world), Some(grid)) = (
        runtime.predicted_local.as_mut(),
        runtime.world.as_ref(),
        runtime.world_grid.as_ref(),
    ) {
        predicted.apply_input(input);
        predicted.simulate_with_grid(delta_seconds, world, grid);
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
            runtime.push_error_message(format!("movement send failed: {error}"));
        }
    }
}

fn movement_direction_from_keys(
    keys: &ButtonInput<KeyCode>,
    accepts_movement_input: bool,
) -> Vec3Net {
    if !accepts_movement_input {
        return Vec3Net::ZERO;
    }

    let mut direction = Vec3Net::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        direction.z += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        direction.z -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        direction.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        direction.x += 1.0;
    }
    direction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_open_ignores_directional_movement_input() {
        let mut keys = ButtonInput::default();
        keys.press(KeyCode::KeyW);
        keys.press(KeyCode::KeyD);

        assert_eq!(
            movement_direction_from_keys(&keys, true),
            Vec3Net::new(1.0, 0.0, 1.0)
        );
        assert_eq!(movement_direction_from_keys(&keys, false), Vec3Net::ZERO);
    }
}
