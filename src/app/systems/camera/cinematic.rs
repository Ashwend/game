//! Detached cinematic camera: while `MenuState.cinematic` is set, the main
//! camera stops following the player and flies the authored shot paths from
//! the shared script (`crate::cinematic`). The countdown and intermission
//! phases park on a still frame (the upcoming shot's opening pose, or the
//! previous shot's final pose) so the operator always sees the framing and
//! every cut point is a hold. The `ViewmodelCamera` is a child of the main
//! camera and follows for free; the follow system stands down on its own
//! cinematic gate (see `camera_follow_system`).

use bevy::prelude::*;

use crate::app::{
    scene::{MainCamera, NetworkPlayer},
    state::{ClientRuntime, MenuState, Screen},
};
use crate::cinematic::script;

type CinematicCameraFilter = (With<MainCamera>, Without<NetworkPlayer>);

/// Advance the overlay's local phase clock. Runs every frame while in-game
/// (simulation never pauses, and the countdown display, camera path time,
/// and intermission chip all derive from this clock). Leaving the in-game
/// screen mid-playback (quit, kick, disconnect) drops the overlay so a
/// later session can never start with stale cinematic gating.
pub(crate) fn tick_cinematic_overlay_system(mut menu: ResMut<MenuState>, time: Res<Time>) {
    if menu.screen != Screen::InGame {
        if menu.cinematic.is_some() {
            menu.cinematic = None;
        }
        return;
    }
    if let Some(overlay) = menu.cinematic.as_mut() {
        overlay.elapsed += time.delta_secs();
    }
}

/// Write the camera transform from the active shot's authored path. Runs in
/// `PostUpdate` before transform propagation, after the (stood-down) follow
/// writer, mirroring `menu_backdrop_camera_system`'s ownership pattern.
pub(crate) fn cinematic_camera_system(
    menu: Res<MenuState>,
    runtime: Res<ClientRuntime>,
    mut camera: Query<&'static mut Transform, CinematicCameraFilter>,
) {
    if menu.screen != Screen::InGame {
        return;
    }
    let Some(overlay) = &menu.cinematic else {
        return;
    };
    let Ok(mut camera_transform) = camera.single_mut() else {
        return;
    };
    let (shot_index, path_time) = overlay.camera_target();
    let Some(shot) = script::shot(shot_index) else {
        return;
    };
    let (eye, mut look) = shot.camera.sample(path_time);
    // Meteor-tracking shots aim at the live fireball while it is in visible
    // flight, evaluated off the same shared trajectory function the sky
    // renderer uses, so the descent is centred whatever its seeded entry
    // azimuth. Before entry and after impact the keyed look applies (the
    // keys already settle on the crater).
    if shot.track_meteor
        && let Some(tracked) = tracked_meteor_position(&runtime)
    {
        look = tracked;
    }
    // Guard the degenerate eye == look case (a bad key) rather than glitch.
    if (look - eye).length_squared() < 1e-6 {
        return;
    }
    *camera_transform = Transform::from_translation(eye).looking_at(look, Vec3::Y);
}

/// World position of the first meteor currently in visible flight, if any.
fn tracked_meteor_position(runtime: &ClientRuntime) -> Option<Vec3> {
    let now = runtime.server_tick_precise();
    runtime.meteor_showers.iter().find_map(|event| {
        crate::world::meteor_world_state(
            Vec2::new(event.impact_position.x, event.impact_position.z),
            event.impact_tick,
            event.trajectory_seed,
            now,
        )
        .map(|state| state.position)
    })
}
