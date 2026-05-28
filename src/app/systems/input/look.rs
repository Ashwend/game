use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{PrimaryWindow, Window},
};

use crate::{
    app::state::{ClientSettings, DeployablePlacementState, LookState, MenuState},
    controller::MAX_LOOK_PITCH,
};

use super::gating::{gameplay_accepts_controls, primary_window_focused};

/// Hard cap on a single frame's mouse delta. Without it, a stalled frame
/// dumps two frames of accumulated motion into one yaw step, which reads as
/// a sudden "snap" while strafing around a focused object. The cap is well
/// above any normal flick (raw pixels — at 4k a fast flick is ~1500 px),
/// so it only kicks in when a frame genuinely hiccups.
const MAX_MOUSE_DELTA_PER_FRAME: f32 = 2000.0;

pub(crate) fn mouse_look_system(
    accumulated_mouse_motion: Res<AccumulatedMouseMotion>,
    mut look: ResMut<LookState>,
    menu: Res<MenuState>,
    settings: Res<ClientSettings>,
    placement: Res<DeployablePlacementState>,
    mouse: Res<ButtonInput<MouseButton>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    if !gameplay_accepts_controls(&menu, primary_window_focused(&primary_window)) {
        return;
    }

    // Holding right-mouse while a deployable is selected hands the mouse
    // over to ghost rotation (see `placement_input_system`). Freeze the
    // camera so the player can dial in an angle without the view sliding
    // out from under the spot they picked.
    if placement.item_id.is_some() && mouse.pressed(MouseButton::Right) {
        return;
    }

    let delta = accumulated_mouse_motion.delta;
    if delta == Vec2::ZERO {
        return;
    }

    let delta = Vec2::new(
        delta
            .x
            .clamp(-MAX_MOUSE_DELTA_PER_FRAME, MAX_MOUSE_DELTA_PER_FRAME),
        delta
            .y
            .clamp(-MAX_MOUSE_DELTA_PER_FRAME, MAX_MOUSE_DELTA_PER_FRAME),
    );

    let sensitivity = look.sensitivity * settings.input.mouse_sensitivity.clamp(0.25, 3.0);
    let pitch_delta = if settings.input.invert_mouse_y {
        delta.y * sensitivity.y
    } else {
        -delta.y * sensitivity.y
    };
    look.yaw -= delta.x * sensitivity.x;
    look.pitch = (look.pitch + pitch_delta).clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
}
