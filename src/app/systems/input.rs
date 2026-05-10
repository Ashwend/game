use bevy::{
    input::mouse::{AccumulatedMouseMotion, MouseWheel},
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow, Window, WindowFocused},
};

use crate::{
    controller::MAX_LOOK_PITCH,
    protocol::{
        ACTIONBAR_SLOT_COUNT, ClientMessage, InventoryCommand, ItemContainerSlot,
        MAX_INPUT_DELTA_SECONDS, PlayerInput, PlayerMovement, Vec3Net,
    },
};

use super::super::state::{
    ClientRuntime, ClientSettings, InventoryUiState, LookState, MenuState, PickupTargetState,
    Screen,
};

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
        handle_pause_escape(&mut menu);
    }
}

fn handle_pause_escape(menu: &mut MenuState) {
    if menu.inventory_open {
        menu.inventory_open = false;
        return;
    }

    if menu.pause_options_open {
        menu.pause_open = true;
        menu.pause_options_open = false;
        return;
    }

    menu.pause_open = !menu.pause_open;
    if !menu.pause_open {
        menu.pause_options_open = false;
    } else {
        menu.inventory_open = false;
    }
}

pub(crate) fn toggle_inventory_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut menu: ResMut<MenuState>,
    mut inventory_ui: ResMut<InventoryUiState>,
) {
    if menu.screen != Screen::InGame || menu.pause_open || menu.pause_options_open || menu.chat_open
    {
        return;
    }

    if keys.just_pressed(KeyCode::Tab) {
        menu.inventory_open = !menu.inventory_open;
        if !menu.inventory_open {
            inventory_ui.cancel_drag();
        }
    }
}

pub(crate) fn update_cursor_system(
    mut cursor_options: Single<&mut CursorOptions>,
    menu: Res<MenuState>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    let should_capture = gameplay_accepts_controls(&menu, primary_window_focused(&primary_window));
    cursor_options.visible = !should_capture;
    cursor_options.grab_mode = if should_capture {
        CursorGrabMode::Locked
    } else {
        CursorGrabMode::None
    };
}

pub(crate) fn center_cursor_on_focus_system(
    mut focus_events: MessageReader<WindowFocused>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut primary_window: Query<(Entity, &mut Window), With<PrimaryWindow>>,
) {
    let Ok((window_entity, mut window)) = primary_window.single_mut() else {
        return;
    };

    let mut should_center = false;
    let mut lost_focus = false;
    for event in focus_events.read() {
        if event.window != window_entity {
            continue;
        }
        if event.focused {
            should_center = true;
        } else {
            lost_focus = true;
        }
    }

    if lost_focus {
        keys.reset_all();
    }
    if should_center {
        let center = Vec2::new(window.width() * 0.5, window.height() * 0.5);
        window.set_cursor_position(Some(center));
    }
}

pub(crate) fn mouse_look_system(
    accumulated_mouse_motion: Res<AccumulatedMouseMotion>,
    mut look: ResMut<LookState>,
    menu: Res<MenuState>,
    settings: Res<ClientSettings>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    if !gameplay_accepts_controls(&menu, primary_window_focused(&primary_window)) {
        return;
    }

    let delta = accumulated_mouse_motion.delta;
    if delta == Vec2::ZERO {
        return;
    }

    let sensitivity = look.sensitivity * settings.input.mouse_sensitivity.clamp(0.25, 3.0);
    let pitch_delta = if settings.input.invert_mouse_y {
        delta.y * sensitivity.y
    } else {
        -delta.y * sensitivity.y
    };
    look.yaw -= delta.x * sensitivity.x;
    look.pitch = (look.pitch + pitch_delta).clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
}

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
    let delta_seconds = time.delta_secs().clamp(0.0, MAX_INPUT_DELTA_SECONDS);
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

pub(crate) fn gameplay_inventory_shortcuts_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut mouse_wheel: MessageReader<MouseWheel>,
    mut runtime: ResMut<ClientRuntime>,
    menu: Res<MenuState>,
    pickup_target: Res<PickupTargetState>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    if !gameplay_accepts_controls(&menu, primary_window_focused(&primary_window)) {
        mouse_wheel.clear();
        return;
    }

    for slot in 0..ACTIONBAR_SLOT_COUNT {
        if actionbar_key_pressed(&keys, slot) {
            send_inventory_command(&mut runtime, InventoryCommand::SelectActionbarSlot { slot });
        }
    }

    let wheel_delta = mouse_wheel
        .read()
        .map(|event| event.y.signum() as i8)
        .sum::<i8>();
    if wheel_delta != 0 {
        send_inventory_command(
            &mut runtime,
            InventoryCommand::SelectActionbarOffset {
                offset: -wheel_delta.signum(),
            },
        );
    }

    if keys.just_pressed(KeyCode::KeyQ) {
        let Some(active_actionbar_slot) = runtime
            .local_player()
            .map(|player| player.inventory.active_actionbar_slot)
        else {
            return;
        };
        send_inventory_command(
            &mut runtime,
            InventoryCommand::Drop {
                from: ItemContainerSlot::actionbar(active_actionbar_slot),
                quantity: Some(1),
            },
        );
    }

    if keys.just_pressed(KeyCode::KeyE)
        && let Some(dropped_item_id) = pickup_target.dropped_item_id
    {
        send_inventory_command(&mut runtime, InventoryCommand::PickUp { dropped_item_id });
    }
}

fn actionbar_key_pressed(keys: &ButtonInput<KeyCode>, slot: usize) -> bool {
    match slot {
        0 => keys.just_pressed(KeyCode::Digit1),
        1 => keys.just_pressed(KeyCode::Digit2),
        2 => keys.just_pressed(KeyCode::Digit3),
        3 => keys.just_pressed(KeyCode::Digit4),
        4 => keys.just_pressed(KeyCode::Digit5),
        5 => keys.just_pressed(KeyCode::Digit6),
        6 => keys.just_pressed(KeyCode::Digit7),
        7 => keys.just_pressed(KeyCode::Digit8),
        8 => keys.just_pressed(KeyCode::Digit9),
        _ => false,
    }
}

pub(crate) fn send_inventory_command(runtime: &mut ClientRuntime, command: InventoryCommand) {
    let Some(session) = runtime.session.as_mut() else {
        runtime.push_error_message("inventory command failed: not connected");
        return;
    };

    if let Err(error) = session.send(ClientMessage::Inventory(command)) {
        runtime.push_error_message(format!("inventory command failed: {error}"));
    }
}

fn primary_window_focused(primary_window: &Query<&Window, With<PrimaryWindow>>) -> bool {
    primary_window
        .single()
        .map(|window| window.focused)
        .unwrap_or(true)
}

fn gameplay_simulation_allowed(menu: &MenuState) -> bool {
    menu.screen == Screen::InGame && !menu.pause_options_open && !menu.chat_open
}

fn gameplay_accepts_controls(menu: &MenuState, window_focused: bool) -> bool {
    window_focused && gameplay_simulation_allowed(menu) && !menu.pause_open && !menu.inventory_open
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_closes_pause_options_back_to_pause_menu() {
        let mut menu = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            pause_options_open: true,
            ..Default::default()
        };

        handle_pause_escape(&mut menu);

        assert!(menu.pause_open);
        assert!(!menu.pause_options_open);
    }

    #[test]
    fn escape_toggles_pause_root_and_clears_nested_options_when_closed() {
        let mut menu = MenuState {
            screen: Screen::InGame,
            ..Default::default()
        };

        handle_pause_escape(&mut menu);
        assert!(menu.pause_open);

        menu.pause_options_open = true;
        handle_pause_escape(&mut menu);
        assert!(menu.pause_open);
        assert!(!menu.pause_options_open);

        handle_pause_escape(&mut menu);
        assert!(!menu.pause_open);
        assert!(!menu.pause_options_open);
    }

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

    #[test]
    fn unfocused_gameplay_blocks_controls_without_blocking_simulation() {
        let menu = MenuState {
            screen: Screen::InGame,
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(gameplay_accepts_controls(&menu, true));
        assert!(!gameplay_accepts_controls(&menu, false));
    }

    #[test]
    fn pause_options_block_gameplay_simulation_and_controls() {
        let menu = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            pause_options_open: true,
            ..Default::default()
        };

        assert!(!gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_controls(&menu, true));
    }

    #[test]
    fn pause_menu_blocks_controls_without_blocking_gameplay_simulation() {
        let menu = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_controls(&menu, true));
    }

    #[test]
    fn inventory_blocks_controls_without_blocking_simulation() {
        let menu = MenuState {
            screen: Screen::InGame,
            inventory_open: true,
            ..Default::default()
        };

        assert!(gameplay_simulation_allowed(&menu));
        assert!(!gameplay_accepts_controls(&menu, true));
    }
}
