//! Test-mode glue: applies [`TestModeConfig`] overrides exactly once per
//! session, and reposition the OS window to the centered tile slot once the
//! primary monitor has been detected.
//!
//! Production runs never set the underlying env vars so both systems
//! short-circuit immediately.

use bevy::{
    prelude::*,
    window::{Monitor, PrimaryMonitor, PrimaryWindow, Window, WindowPosition},
};

use crate::{
    app::state::{ClientRuntime, LookState, MenuState, Screen, TestModeConfig},
    protocol::ClientMessage,
};

pub(crate) fn apply_test_mode_overrides_system(
    config: Res<TestModeConfig>,
    mut runtime: ResMut<ClientRuntime>,
    mut menu: ResMut<MenuState>,
    mut look: ResMut<LookState>,
    mut already_applied: Local<bool>,
) {
    if *already_applied || !config.has_runtime_overrides() {
        return;
    }
    if menu.screen != Screen::InGame {
        return;
    }
    let Some(predicted) = runtime.predicted_local.as_mut() else {
        return;
    };

    // Movement is client-authoritative: bump the predicted controller's
    // pose and the server will accept the next outbound packet at this
    // new pose. Yaw lives in two places — on the controller (snapshot
    // round-trip + remote rendering) and on `LookState` (the camera + the
    // next outbound input). Set both so the override isn't immediately
    // clobbered by `client_input_system` reading `LookState.yaw` and
    // writing it back through `apply_input`.
    predicted.position.x += config.spawn_offset_x;
    predicted.position.z += config.spawn_offset_z;
    if let Some(yaw) = config.spawn_yaw {
        predicted.yaw = yaw;
        look.yaw = yaw;
    }

    if config.inventory_open_on_join {
        menu.inventory_open = true;
    }

    if config.auto_test_kit_on_join
        && let Some(session) = runtime.session.as_mut()
    {
        // Fire-and-forget: any send error here is debug ergonomics
        // only — the player can re-issue `/test-kit` from chat if it
        // ever fails to land.
        let _ = session.send(ClientMessage::Command {
            text: "test-kit".to_owned(),
        });
    }

    *already_applied = true;
}

/// Repositions the primary window into its centered tile slot once Bevy
/// has surfaced the primary monitor. We can't compute the position at
/// startup because monitor dimensions aren't available before the window
/// is created — so the window opens at whatever winit's default chose,
/// and this system snaps it into place on the first frame the monitor is
/// queryable. Runs once per session (gated by `Local<bool>`).
pub(crate) fn reposition_test_window_system(
    config: Res<TestModeConfig>,
    monitors: Query<&Monitor, With<PrimaryMonitor>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut applied: Local<bool>,
) {
    if *applied {
        return;
    }
    let Some(layout) = config.window else {
        // No test layout requested — leave winit's default placement
        // alone and stop polling.
        *applied = true;
        return;
    };
    let Ok(monitor) = monitors.single() else {
        return;
    };
    let Ok(mut window) = windows.single_mut() else {
        return;
    };

    // Bevy reports the monitor in *physical* pixels, but `Window.position`
    // is in *logical* pixels — divide by the scale factor so the math
    // matches on Retina/HiDPI displays. `scale_factor` is f64 in Bevy;
    // cast to f32 for the divide.
    let scale = (monitor.scale_factor as f32).max(1.0);
    let logical_width = (monitor.physical_width as f32 / scale) as u32;
    let logical_height = (monitor.physical_height as f32 / scale) as u32;
    let mut position = layout.position_in_screen(UVec2::new(logical_width, logical_height));
    // Account for the monitor's offset on multi-display setups so the
    // window lands on the *primary* monitor's rect rather than the
    // virtual-desktop origin.
    position += monitor.physical_position;

    window.position = WindowPosition::At(position);
    *applied = true;
}
