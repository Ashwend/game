use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow, Window, WindowFocused},
};

use crate::{
    controller::MAX_LOOK_PITCH,
    protocol::{ClientMessage, MAX_INPUT_DELTA_SECONDS, PlayerInput, PlayerMovement, Vec3Net},
};

use super::super::state::{ClientRuntime, LookState, MenuState, Screen};

pub(crate) fn chat_shortcut_system(keys: Res<ButtonInput<KeyCode>>, mut menu: ResMut<MenuState>) {
    if menu.screen != Screen::InGame || menu.pause_open || menu.chat_open {
        return;
    }

    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::KeyT) {
        menu.chat_open = true;
        menu.chat_focus_pending = true;
        menu.chat_input.clear();
    }
}

pub(crate) fn toggle_pause_system(keys: Res<ButtonInput<KeyCode>>, mut menu: ResMut<MenuState>) {
    if menu.screen != Screen::InGame {
        return;
    }
    if menu.chat_open {
        return;
    }

    if keys.just_pressed(KeyCode::Escape) {
        menu.pause_open = !menu.pause_open;
    }
}

pub(crate) fn update_cursor_system(
    mut cursor_options: Single<&mut CursorOptions>,
    menu: Res<MenuState>,
) {
    let should_capture = menu.screen == Screen::InGame && !menu.pause_open && !menu.chat_open;
    cursor_options.visible = !should_capture;
    cursor_options.grab_mode = if should_capture {
        CursorGrabMode::Locked
    } else {
        CursorGrabMode::None
    };
}

pub(crate) fn center_cursor_on_focus_system(
    mut focus_events: MessageReader<WindowFocused>,
    mut primary_window: Query<(Entity, &mut Window), With<PrimaryWindow>>,
) {
    let Ok((window_entity, mut window)) = primary_window.single_mut() else {
        return;
    };

    let should_center = focus_events
        .read()
        .any(|event| event.window == window_entity && event.focused);
    if !should_center {
        return;
    }

    let center = Vec2::new(window.width() * 0.5, window.height() * 0.5);
    window.set_cursor_position(Some(center));
}

pub(crate) fn mouse_look_system(
    accumulated_mouse_motion: Res<AccumulatedMouseMotion>,
    mut look: ResMut<LookState>,
    menu: Res<MenuState>,
) {
    if menu.screen != Screen::InGame || menu.pause_open || menu.chat_open {
        return;
    }

    let delta = accumulated_mouse_motion.delta;
    if delta == Vec2::ZERO {
        return;
    }

    look.yaw -= delta.x * look.sensitivity.x;
    look.pitch = (look.pitch - delta.y * look.sensitivity.y).clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
}

pub(crate) fn client_input_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut runtime: ResMut<ClientRuntime>,
    menu: Res<MenuState>,
    look: Res<LookState>,
) {
    if menu.screen != Screen::InGame || menu.pause_open || menu.chat_open {
        return;
    }
    if runtime.client_id.is_none() {
        return;
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

    runtime.input_sequence += 1;
    let sequence = runtime.input_sequence;
    let delta_seconds = time.delta_secs().clamp(0.0, MAX_INPUT_DELTA_SECONDS);
    let input = PlayerInput {
        sequence,
        delta_seconds,
        direction,
        sprint: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        jump: keys.just_pressed(KeyCode::Space),
        yaw: look.yaw,
        pitch: look.pitch,
    };

    let mut movement = None;
    let world = runtime.world.clone();
    if let (Some(predicted), Some(world)) = (runtime.predicted_local.as_mut(), world.as_ref()) {
        predicted.apply_input(input);
        predicted.simulate(delta_seconds, world);
        movement = Some(PlayerMovement {
            sequence,
            position: predicted.position,
            velocity: predicted.velocity,
            yaw: predicted.yaw,
            pitch: predicted.pitch,
            grounded: predicted.grounded,
        });
    }

    if let (Some(session), Some(movement)) = (runtime.session.as_mut(), movement) {
        let _ = session.send(ClientMessage::Movement(movement));
    }
}
