use bevy::prelude::*;

use crate::app::{
    EYE_HEIGHT,
    scene::{MainCamera, NetworkPlayer},
    state::{ClientRuntime, MenuState, Screen},
};

use super::effects::{CameraImpactKick, CameraMotionEffects};

type FollowCameraData = (&'static mut Transform, Option<&'static mut Projection>);
type FollowCameraFilter = (With<MainCamera>, Without<NetworkPlayer>);

pub(crate) fn camera_follow_system(
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    time: Res<Time>,
    mut kick: ResMut<CameraImpactKick>,
    mut motion: ResMut<CameraMotionEffects>,
    mut camera: Query<FollowCameraData, FollowCameraFilter>,
) {
    if menu.screen != Screen::InGame {
        motion.reset();
        return;
    }

    let Ok((mut camera_transform, projection)) = camera.single_mut() else {
        return;
    };
    let Some(player) = runtime.local_view() else {
        motion.reset();
        return;
    };

    let (pitch_kick, down_kick) = kick.advance(time.delta_secs());

    let (horizontal_speed, fall_speed, grounded) = runtime
        .predicted_local
        .as_ref()
        .map(|controller| {
            let vx = controller.velocity.x;
            let vz = controller.velocity.z;
            let speed = vx.mul_add(vx, vz * vz).sqrt();
            // Positive fall_speed = falling. Velocity.y is downward when
            // negative, so we negate (clamped to 0 on the way up).
            let fall = (-controller.velocity.y).max(0.0);
            (speed, fall, controller.grounded)
        })
        .unwrap_or((0.0, 0.0, true));

    motion.advance(time.delta_secs(), horizontal_speed, grounded, fall_speed);

    let feet = Vec3::from(player.position);
    let eye = feet + Vec3::Y * EYE_HEIGHT;
    let bob_y = motion.bob_offset_y();
    let dip_y = motion.landing_dip_y();
    let base_rotation = Quat::from_euler(EulerRot::YXZ, player.yaw, player.pitch, 0.0);
    let rotation = base_rotation * Quat::from_rotation_x(-pitch_kick);
    // Apply the downward drop in world space — feels like the shoulders
    // absorbing the strike without the camera diving along the look vector.
    // Head bob and landing dip stack on the same axis: bob adds a small
    // periodic offset, dip pulls down briefly on touchdown.
    camera_transform.translation = eye + Vec3::Y * (bob_y - dip_y - down_kick);
    camera_transform.rotation = rotation;

    if let Some(mut projection) = projection
        && let Projection::Perspective(perspective) = projection.as_mut()
    {
        perspective.fov = motion.fov_radians();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        controller::PlayerController,
        protocol::{MAX_HEALTH, PlayerState, Vec3Net},
    };

    fn player_state(position: Vec3Net, yaw: f32, pitch: f32) -> PlayerState {
        PlayerState {
            client_id: 1,
            steam_id: 1,
            name: "Player 1".to_owned(),
            position,
            velocity: Vec3Net::ZERO,
            yaw,
            pitch,
            health: MAX_HEALTH,
            grounded: true,
            last_processed_input: 0,
            is_admin: false,
            chat_bubble: None,
            inventory: None,
        }
    }

    #[test]
    fn gameplay_camera_uses_predicted_pose_as_single_source() {
        let mut app = App::new();
        let yaw = 0.8;
        let pitch = -0.2;
        app.insert_resource(MenuState {
            screen: Screen::InGame,
            ..Default::default()
        });
        app.insert_resource(ClientRuntime {
            predicted_local: Some(PlayerController::from_player_state(&player_state(
                Vec3Net::new(2.0, 1.0, -3.0),
                yaw,
                pitch,
            ))),
            ..Default::default()
        });
        app.insert_resource(Time::<()>::default());
        app.insert_resource(CameraImpactKick::default());
        app.insert_resource(CameraMotionEffects::default());
        app.world_mut().spawn((
            MainCamera,
            Transform::from_xyz(0.0, 0.0, 0.0),
            GlobalTransform::default(),
        ));
        app.add_systems(Update, camera_follow_system);

        app.update();

        let mut query = app
            .world_mut()
            .query_filtered::<&Transform, With<MainCamera>>();
        let transform = query.single(app.world()).expect("camera transform");
        assert_eq!(
            transform.translation,
            Vec3::new(2.0, 1.0 + EYE_HEIGHT, -3.0)
        );
        assert!(
            transform
                .rotation
                .abs_diff_eq(Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0), 0.0001)
        );
    }
}
