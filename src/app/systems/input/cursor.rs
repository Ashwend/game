use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow, Window, WindowFocused},
};

use crate::app::state::MenuState;

use super::gating::{gameplay_accepts_controls, primary_window_focused};

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
