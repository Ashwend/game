//! Applies the player's [`GraphicsSettings`](crate::app::state::GraphicsSettings)
//! to the live camera. Mirrors the `apply_display_settings_system` pattern: it
//! is change-gated on `ClientSettings` so it only does work on the frame a
//! setting actually moves, and on the first frame after the resource exists.
//!
//! Ownership split: this system owns the camera's [`Bloom`] component and the
//! sun's shadow config (`DirectionalLight::shadows_enabled` + the cascade
//! config + the global shadow map resolution). MSAA is intentionally *not*
//! touched here, the `menu_backdrop_camera_system` already owns the `Msaa`
//! slot (it swaps it between the menu and in-game), so it reads the MSAA
//! setting directly to keep a single writer of that component. The sky system
//! writes the sun's colour/illuminance/transform but never `shadows_enabled`,
//! so there's no contention on the shared `DirectionalLight`.

use bevy::{
    light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap},
    post_process::bloom::Bloom,
    prelude::*,
};

use crate::app::{
    scene::{MainCamera, SunLight},
    state::ClientSettings,
};

pub(crate) fn apply_graphics_settings_system(
    settings: Res<ClientSettings>,
    mut commands: Commands,
    camera: Query<(Entity, Option<&Bloom>), With<MainCamera>>,
    mut sun: Query<(Entity, &mut DirectionalLight), With<SunLight>>,
    shadow_map: Option<ResMut<DirectionalLightShadowMap>>,
) {
    if !settings.is_changed() {
        return;
    }
    let Ok((entity, bloom)) = camera.single() else {
        return;
    };

    // Bloom is a fixed-strength on/off. Slightly above the `Bloom::NATURAL`
    // preset (0.15 -> 0.20) so the grass's HDR tip glow and the sun haze read as
    // a soft dreamy bloom closer to the stylized reference. No exposed slider.
    if settings.graphics.bloom_enabled {
        if bloom.is_none() {
            commands.entity(entity).insert(Bloom {
                intensity: 0.20,
                ..Bloom::NATURAL
            });
        }
    } else if bloom.is_some() {
        // `Bloom` requires `Hdr`, but removing `Bloom` leaves `Hdr` in place
        // (required components aren't auto-removed), exactly what we want,
        // since the atmosphere sky needs HDR regardless of the bloom toggle.
        commands.entity(entity).remove::<Bloom>();
    }

    // Sun shadows: disabling them, or shrinking the cascade distance / count /
    // map resolution, is the biggest GPU lever in dense forest (every tree
    // re-renders into each cascade). `High` matches the engine defaults.
    if let Ok((sun_entity, mut light)) = sun.single_mut() {
        let config = settings.graphics.shadows.config();
        if light.shadows_enabled != config.is_some() {
            light.shadows_enabled = config.is_some();
        }
        if let Some(cfg) = config {
            commands.entity(sun_entity).insert(
                CascadeShadowConfigBuilder {
                    num_cascades: cfg.num_cascades,
                    maximum_distance: cfg.maximum_distance,
                    first_cascade_far_bound: 8.0,
                    ..default()
                }
                .build(),
            );
            if let Some(mut shadow_map) = shadow_map
                && shadow_map.size != cfg.map_size
            {
                shadow_map.size = cfg.map_size;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_camera() -> App {
        let mut app = App::new();
        app.insert_resource(ClientSettings::default());
        app.world_mut().spawn((MainCamera, Camera3d::default()));
        app.add_systems(Update, apply_graphics_settings_system);
        app
    }

    #[test]
    fn default_settings_attach_bloom_to_camera() {
        let mut app = app_with_camera();
        app.update();

        let bloom = app
            .world_mut()
            .query_filtered::<&Bloom, With<MainCamera>>()
            .single(app.world())
            .expect("default graphics settings enable bloom");
        assert_eq!(bloom.intensity, 0.20);
    }

    #[test]
    fn disabling_bloom_removes_the_component() {
        let mut app = app_with_camera();
        app.update();

        app.world_mut()
            .resource_mut::<ClientSettings>()
            .graphics
            .bloom_enabled = false;
        app.update();

        let has_bloom = app
            .world_mut()
            .query_filtered::<&Bloom, With<MainCamera>>()
            .single(app.world())
            .is_ok();
        assert!(!has_bloom, "disabling bloom should remove the component");
    }

    #[test]
    fn shadow_quality_drives_the_sun_light() {
        use crate::app::state::ShadowQuality;

        let mut app = App::new();
        app.insert_resource(ClientSettings::default());
        app.insert_resource(DirectionalLightShadowMap::default());
        app.world_mut().spawn((MainCamera, Camera3d::default()));
        let sun = app
            .world_mut()
            .spawn((
                SunLight,
                DirectionalLight {
                    shadows_enabled: true,
                    ..default()
                },
            ))
            .id();
        app.add_systems(Update, apply_graphics_settings_system);

        // Default (High) keeps sun shadows on at the 2048 map size.
        app.update();
        assert!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .shadows_enabled,
            "High keeps shadows on"
        );
        assert_eq!(
            app.world().resource::<DirectionalLightShadowMap>().size,
            2048
        );

        // Off disables the sun's shadows entirely.
        app.world_mut()
            .resource_mut::<ClientSettings>()
            .graphics
            .shadows = ShadowQuality::Off;
        app.update();
        assert!(
            !app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .shadows_enabled,
            "Off disables shadows"
        );

        // Low re-enables them at the cheaper 1024 map size.
        app.world_mut()
            .resource_mut::<ClientSettings>()
            .graphics
            .shadows = ShadowQuality::Low;
        app.update();
        assert!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .shadows_enabled
        );
        assert_eq!(
            app.world().resource::<DirectionalLightShadowMap>().size,
            1024
        );
    }
}
