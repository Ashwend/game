//! Applies the player's [`GraphicsSettings`](crate::app::state::GraphicsSettings)
//! to the live camera. Mirrors the `apply_display_settings_system` pattern: it
//! is change-gated on `ClientSettings` so it only does work on the frame a
//! setting actually moves, and on the first frame after the resource exists.
//!
//! Ownership split: this system owns the camera's [`Bloom`] component, the
//! atmosphere/sky quality (`AtmosphereSettings` + `AtmosphereEnvironmentMapLight`),
//! and the sun's shadow config (`DirectionalLight::shadow_maps_enabled`,
//! `soft_shadow_size` for PCSS, the cascade config, and the global shadow map
//! resolution). MSAA / FXAA / TAA are intentionally *not* touched here, the
//! `menu_backdrop_camera_system` already owns those camera AA components (it
//! swaps `Msaa` between the menu and in-game) to keep a single writer. The sky
//! system writes the sun's colour/illuminance/transform but never the shadow
//! fields, so there's no contention on the shared `DirectionalLight`.

use bevy::{
    light::{AtmosphereEnvironmentMapLight, DirectionalLightShadowMap},
    pbr::AtmosphereSettings,
    post_process::bloom::Bloom,
    prelude::*,
};

use crate::app::{
    scene::{MainCamera, SUN_SOFT_SHADOW_SIZE, SunLight},
    state::ClientSettings,
};

type CameraQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static Bloom>,
        Option<&'static mut AtmosphereSettings>,
        Option<&'static mut AtmosphereEnvironmentMapLight>,
    ),
    With<MainCamera>,
>;

pub(crate) fn apply_graphics_settings_system(
    settings: Res<ClientSettings>,
    mut commands: Commands,
    mut camera: CameraQuery,
    mut sun: Query<(Entity, &mut DirectionalLight), With<SunLight>>,
    shadow_map: Option<ResMut<DirectionalLightShadowMap>>,
) {
    if !settings.is_changed() {
        return;
    }
    let Ok((entity, bloom, atmosphere, env_map)) = camera.single_mut() else {
        return;
    };

    // Bloom is a fixed-strength on/off. Kept just below the `Bloom::NATURAL`
    // preset (0.15 -> 0.11): enough that the sun disc and the grass's HDR tip
    // glow still bloom softly, but not the heavy wash that made the whole frame
    // read hazy/dreamy. No exposed slider.
    if settings.graphics.bloom_enabled && settings.dev.bloom {
        if bloom.is_none() {
            commands.entity(entity).insert(Bloom {
                intensity: 0.11,
                ..Bloom::NATURAL
            });
        }
    } else if bloom.is_some() {
        // `Bloom` requires `Hdr`, but removing `Bloom` leaves `Hdr` in place
        // (required components aren't auto-removed), exactly what we want,
        // since the atmosphere sky needs HDR regardless of the bloom toggle.
        commands.entity(entity).remove::<Bloom>();
    }

    // Atmosphere / sky quality. The scattering LUTs and the IBL cubemap are
    // refiltered every frame in Bevy 0.18, so their sample counts and the cubemap
    // size are a direct per-frame GPU cost. Both are live-mutable camera
    // components (no respawn); writing them takes effect next frame. (Optional in
    // the query so the unit-test camera, spawned without them, still matches.)
    let atmo = settings.graphics.atmosphere.config();
    if let Some(mut atmosphere) = atmosphere {
        atmosphere.transmittance_lut_samples = atmo.transmittance_lut_samples;
        atmosphere.multiscattering_lut_samples = atmo.multiscattering_lut_samples;
        atmosphere.sky_view_lut_size =
            UVec2::new(atmo.sky_view_lut_size.0, atmo.sky_view_lut_size.1);
        atmosphere.sky_view_lut_samples = atmo.sky_view_lut_samples;
        atmosphere.aerial_view_lut_samples = atmo.aerial_view_lut_samples;
    }
    if let Some(mut env_map) = env_map {
        let size = UVec2::splat(atmo.env_map_size);
        if env_map.size != size {
            env_map.size = size;
        }
        // Dev: the `Atmosphere ambient` toggle kills the sky's image-based
        // ambient/reflection fill entirely (so you can see the scene lit by the sun
        // + ambient floor alone); when on, the live `DevLighting` slider sets the
        // intensity. Both default to the shipped `ATMOSPHERE_AMBIENT_INTENSITY`.
        let intensity = if settings.dev.atmosphere_ibl {
            settings.dev.lighting.atmosphere_ibl_intensity
        } else {
            0.0
        };
        if env_map.intensity != intensity {
            env_map.intensity = intensity;
        }
    }

    // Sun shadows: disabling them, or shrinking the cascade distance / count /
    // map resolution, is the biggest GPU lever in dense forest (every tree
    // re-renders into each cascade). `High` matches the engine defaults.
    if let Ok((sun_entity, mut light)) = sun.single_mut() {
        // Dev override: `sun_shadows` off forces the cascade config to `None`
        // (shadows disabled) regardless of the Graphics quality tier.
        let config = if settings.dev.sun_shadows {
            settings.graphics.shadows.config()
        } else {
            None
        };
        if light.shadow_maps_enabled != config.is_some() {
            light.shadow_maps_enabled = config.is_some();
        }
        // PCSS soft shadows: a distance-widening penumbra when both shadows and
        // the soft-shadow toggle are on; `None` falls back to the hard PCF path.
        // (Needs the `experimental_pbr_pcss` feature, already enabled in Cargo.)
        let soft =
            if config.is_some() && settings.graphics.soft_shadows && settings.dev.soft_shadows {
                Some(SUN_SOFT_SHADOW_SIZE)
            } else {
                None
            };
        if light.soft_shadow_size != soft {
            light.soft_shadow_size = soft;
        }
        if let Some(cfg) = config {
            commands.entity(sun_entity).insert(cfg.cascade_config());
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
        assert_eq!(bloom.intensity, 0.11);
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
                    shadow_maps_enabled: true,
                    ..default()
                },
            ))
            .id();
        app.add_systems(Update, apply_graphics_settings_system);

        // Default (Ultra) keeps sun shadows on at the 4096 map size.
        app.update();
        assert!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .shadow_maps_enabled,
            "Ultra keeps shadows on"
        );
        assert_eq!(
            app.world().resource::<DirectionalLightShadowMap>().size,
            4096
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
                .shadow_maps_enabled,
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
                .shadow_maps_enabled
        );
        assert_eq!(
            app.world().resource::<DirectionalLightShadowMap>().size,
            1024
        );
    }

    fn app_with_sun() -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(ClientSettings::default());
        app.insert_resource(DirectionalLightShadowMap::default());
        app.world_mut().spawn((MainCamera, Camera3d::default()));
        let sun = app
            .world_mut()
            .spawn((
                SunLight,
                DirectionalLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
            ))
            .id();
        app.add_systems(Update, apply_graphics_settings_system);
        (app, sun)
    }

    #[test]
    fn soft_shadows_setting_toggles_pcss() {
        use crate::app::state::ShadowQuality;
        let (mut app, sun) = app_with_sun();

        // Default: shadows High + soft_shadows on -> a PCSS penumbra is set.
        app.update();
        assert!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .soft_shadow_size
                .is_some(),
            "default soft shadows enable PCSS"
        );

        // Turning soft shadows off falls back to the hard PCF path.
        app.world_mut()
            .resource_mut::<ClientSettings>()
            .graphics
            .soft_shadows = false;
        app.update();
        assert!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .soft_shadow_size
                .is_none(),
            "disabling soft shadows clears PCSS"
        );

        // With shadows Off, PCSS is moot regardless of the toggle.
        let mut settings = app.world_mut().resource_mut::<ClientSettings>();
        settings.graphics.soft_shadows = true;
        settings.graphics.shadows = ShadowQuality::Off;
        app.update();
        assert!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .soft_shadow_size
                .is_none(),
            "no PCSS when shadows are off"
        );
    }

    #[test]
    fn ultra_uses_a_4096_shadow_map() {
        use crate::app::state::ShadowQuality;
        let (mut app, _sun) = app_with_sun();

        app.world_mut()
            .resource_mut::<ClientSettings>()
            .graphics
            .shadows = ShadowQuality::Ultra;
        app.update();
        assert_eq!(
            app.world().resource::<DirectionalLightShadowMap>().size,
            4096
        );
    }
}
