use bevy::post_process::dof::DepthOfField;
use bevy::prelude::*;

use super::super::{
    EYE_HEIGHT,
    scene::{MainCamera, NetworkPlayer, menu_backdrop_depth_of_field},
    state::{ClientRuntime, LookState, MenuState, Screen},
};

const MENU_BACKDROP_EYE: Vec3 = Vec3::new(-5.8, EYE_HEIGHT, 7.2);
const MENU_BACKDROP_LOOK_AT: Vec3 = Vec3::new(0.4, 0.85, -3.6);
const MENU_BACKDROP_PAN_SPEED: f32 = 0.055;
const MENU_BACKDROP_PAN_RADIUS: Vec3 = Vec3::new(0.42, 0.035, 0.32);
const MENU_BACKDROP_LOOK_RADIUS: Vec3 = Vec3::new(0.22, 0.03, 0.18);

type MenuBackdropCameraData = (
    Entity,
    &'static mut Transform,
    Option<&'static mut DepthOfField>,
);
type MenuBackdropCameraFilter = (With<MainCamera>, Without<NetworkPlayer>);

pub(crate) fn menu_backdrop_camera_system(
    mut commands: Commands,
    menu: Res<MenuState>,
    time: Option<Res<Time>>,
    mut camera: Query<MenuBackdropCameraData, MenuBackdropCameraFilter>,
) {
    let Ok((entity, mut camera_transform, depth_of_field)) = camera.single_mut() else {
        return;
    };

    if menu.screen == Screen::InGame {
        if depth_of_field.is_some() {
            commands.entity(entity).remove::<DepthOfField>();
        }
        return;
    }

    let elapsed_seconds = time
        .as_ref()
        .map(|time| time.elapsed_secs())
        .unwrap_or_default();
    *camera_transform = menu_backdrop_transform(elapsed_seconds);
    if let Some(mut depth_of_field) = depth_of_field {
        *depth_of_field = menu_backdrop_depth_of_field();
    } else {
        commands
            .entity(entity)
            .insert(menu_backdrop_depth_of_field());
    }
}

pub(crate) fn camera_follow_system(
    runtime: Res<ClientRuntime>,
    look: Res<LookState>,
    menu: Res<MenuState>,
    mut camera: Query<&mut Transform, (With<MainCamera>, Without<NetworkPlayer>)>,
) {
    if menu.screen != Screen::InGame {
        return;
    }

    let Ok(mut camera_transform) = camera.single_mut() else {
        return;
    };
    let Some(player) = runtime.local_view() else {
        return;
    };

    let feet = Vec3::new(player.position.x, player.position.y, player.position.z);
    let eye = feet + Vec3::Y * EYE_HEIGHT;
    camera_transform.translation = eye;
    camera_transform.rotation = Quat::from_euler(EulerRot::YXZ, look.yaw, look.pitch, 0.0);
}

fn menu_backdrop_transform(elapsed_seconds: f32) -> Transform {
    let phase = elapsed_seconds * MENU_BACKDROP_PAN_SPEED;
    let eye = MENU_BACKDROP_EYE
        + Vec3::new(
            phase.sin() * MENU_BACKDROP_PAN_RADIUS.x,
            (phase * 0.7).sin() * MENU_BACKDROP_PAN_RADIUS.y,
            phase.cos() * MENU_BACKDROP_PAN_RADIUS.z,
        );
    let look_at = MENU_BACKDROP_LOOK_AT
        + Vec3::new(
            (phase * 0.65).cos() * MENU_BACKDROP_LOOK_RADIUS.x,
            (phase * 0.5).sin() * MENU_BACKDROP_LOOK_RADIUS.y,
            (phase * 0.8).sin() * MENU_BACKDROP_LOOK_RADIUS.z,
        );
    Transform::from_translation(eye).looking_at(look_at, Vec3::Y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::post_process::dof::DepthOfFieldMode;

    fn app_with_camera(menu: MenuState) -> App {
        let mut app = App::new();
        app.insert_resource(menu);
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn((
                MainCamera,
                Camera3d::default(),
                menu_backdrop_depth_of_field(),
                Transform::from_xyz(0.0, EYE_HEIGHT, 3.0),
            ));
        });
        app.add_systems(Update, menu_backdrop_camera_system);
        app
    }

    #[test]
    fn menu_backdrop_camera_sets_soft_panning_world_view() {
        let mut app = app_with_camera(MenuState::default());
        app.update();
        app.update();

        let mut query = app
            .world_mut()
            .query_filtered::<(&Transform, &DepthOfField), With<MainCamera>>();
        let (transform, depth_of_field) = query
            .single(app.world())
            .expect("menu camera should have dof");

        assert!(transform.translation.distance(MENU_BACKDROP_EYE) <= 0.6);
        assert_eq!(depth_of_field.mode, DepthOfFieldMode::Gaussian);
        assert!(depth_of_field.aperture_f_stops < 1.0);
    }

    #[test]
    fn gameplay_camera_removes_depth_of_field() {
        let menu = MenuState {
            screen: Screen::InGame,
            ..Default::default()
        };
        let mut app = app_with_camera(menu);
        app.update();
        app.update();

        let mut query = app
            .world_mut()
            .query_filtered::<&DepthOfField, With<MainCamera>>();
        assert!(query.single(app.world()).is_err());
    }
}
