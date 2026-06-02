//! Test-mode glue: applies [`TestModeConfig`] overrides exactly once per
//! session, and reposition the OS window to the centered tile slot once the
//! primary monitor has been detected.
//!
//! Production runs never set the underlying env vars so both systems
//! short-circuit immediately.

use bevy::{
    prelude::*,
    window::{Monitor, MonitorSelection, PrimaryWindow, Window, WindowMode, WindowPosition},
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
    // new pose. Yaw lives in two places, on the controller (snapshot
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
        // only, the player can re-issue `/test-kit` from chat if it
        // ever fails to land.
        let _ = session.send(ClientMessage::Command {
            text: "test-kit".to_owned(),
        });
    }

    *already_applied = true;
}

/// Places the multiplayer-test window on its target display once Bevy has
/// surfaced the monitors. With two or more monitors each client gets its own
/// screen, borderless-fullscreen, `player1` (index 0) on the leftmost
/// monitor, `player2` (index 1) on the next one to the right. With a single
/// monitor it falls back to the centered side-by-side tiling so both windows
/// still fit. Runs once per session (gated by `Local<bool>`).
///
/// We can't decide this at startup because monitor geometry isn't known until
/// after the window opens, so the window comes up plain windowed (see the
/// `WindowPlugin` setup in `app.rs`) and this snaps it into place on the first
/// frame the monitors are queryable.
pub(crate) fn reposition_test_window_system(
    config: Res<TestModeConfig>,
    monitors: Query<(Entity, &Monitor)>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut applied: Local<bool>,
) {
    if *applied {
        return;
    }
    let Some(layout) = config.window else {
        // No test layout requested, leave winit's default placement
        // alone and stop polling.
        *applied = true;
        return;
    };
    if monitors.is_empty() {
        // Monitors not surfaced yet, winit reports them within a frame or
        // two of the window opening; retry until they're queryable.
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };

    // Order monitors left-to-right by their virtual-desktop position so index
    // 0 is the leftmost screen. When two monitors share an x (or we otherwise
    // can't tell them apart) the sort just keeps a stable order, i.e. it
    // falls back to enumeration order, as requested.
    let mut ordered: Vec<(Entity, &Monitor)> = monitors.iter().collect();
    ordered.sort_by_key(|(_, monitor)| (monitor.physical_position.x, monitor.physical_position.y));

    if ordered.len() >= 2 {
        // One client per monitor: fill the assigned screen with no chrome.
        let (target, _) = ordered[(layout.index as usize).min(ordered.len() - 1)];
        window.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Entity(target));
    } else {
        // Single monitor: keep the centered side-by-side windowed tiling.
        // Bevy reports the monitor in *physical* pixels but `Window.position`
        // is *logical*, divide by the scale factor so the math matches on
        // Retina/HiDPI displays (`scale_factor` is f64; cast to f32).
        let (_, monitor) = ordered[0];
        let scale = (monitor.scale_factor as f32).max(1.0);
        let logical = UVec2::new(
            (monitor.physical_width as f32 / scale) as u32,
            (monitor.physical_height as f32 / scale) as u32,
        );
        // Add the monitor's offset so the window lands on its rect rather than
        // the virtual-desktop origin.
        let position = layout.position_in_screen(logical) + monitor.physical_position;
        window.mode = WindowMode::Windowed;
        window.position = WindowPosition::At(position);
    }
    *applied = true;
}

/// Run condition: true while this client is a `multiplayer-test` window, where
/// the test harness, not the player's saved display settings, owns the
/// window. [`apply_display_settings_system`](super::apply_display_settings_system)
/// is gated off in that case so it can't fight
/// [`reposition_test_window_system`], which may put the window
/// borderless-fullscreen on a non-primary monitor. Tolerates a missing
/// resource so unit-test apps that never insert it read as "not a test".
pub(crate) fn multiplayer_test_owns_window(config: Option<Res<TestModeConfig>>) -> bool {
    config.is_some_and(|config| config.window.is_some())
}
