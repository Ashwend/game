use bevy::{
    prelude::*,
    window::{Monitor, MonitorSelection, PrimaryMonitor, PrimaryWindow, Window, WindowPosition},
};
use bevy_framepace::{FramepaceSettings, Limiter};

use super::super::state::{ClientSettings, DisplayMode, DisplayResolution};

const DEFAULT_WINDOWED_WIDTH: u32 = 1280;
const DEFAULT_WINDOWED_HEIGHT: u32 = 720;

pub(crate) fn apply_display_settings_system(
    mut settings: ResMut<ClientSettings>,
    mut primary_window: Query<&mut Window, With<PrimaryWindow>>,
    primary_monitor: Query<&Monitor, With<PrimaryMonitor>>,
    mut framepace: ResMut<FramepaceSettings>,
    mut previous_mode: Local<Option<DisplayMode>>,
) {
    let Ok(mut window) = primary_window.single_mut() else {
        return;
    };
    let primary_monitor = primary_monitor.single().ok();

    let leaving_fullscreen = previous_mode.is_some_and(|mode| mode != DisplayMode::Windowed)
        && settings.display.mode == DisplayMode::Windowed;
    if leaving_fullscreen {
        settings.display.resolution =
            DisplayResolution::new(DEFAULT_WINDOWED_WIDTH, DEFAULT_WINDOWED_HEIGHT);
        window.position = WindowPosition::Centered(MonitorSelection::Primary);
    }
    *previous_mode = Some(settings.display.mode);

    let target_mode = settings.display.window_mode(primary_monitor);

    if window.present_mode != settings.display.present_mode() {
        window.present_mode = settings.display.present_mode();
    }

    // Software-side frame cap follows the vsync toggle. `Limiter::Auto`
    // queries the active monitor's refresh rate; `Limiter::Off` runs
    // uncapped. Cheap comparison every frame, no allocation.
    let target_limiter = settings.display.frame_limiter();
    if !limiters_eq(&framepace.limiter, &target_limiter) {
        framepace.limiter = target_limiter;
    }

    if window.resizable {
        window.resizable = false;
    }

    if window.mode != target_mode {
        window.mode = target_mode;
    }

    if settings.display.mode == DisplayMode::Windowed {
        let resolution = settings.display.resolution;
        if window.resolution.physical_width() != resolution.width
            || window.resolution.physical_height() != resolution.height
        {
            window
                .resolution
                .set_physical_resolution(resolution.width, resolution.height);
        }
    }
}

/// `Limiter` doesn't derive `PartialEq` in `bevy_framepace`. We only
/// ever swap between `Auto` and `Off` — `Manual` isn't used — so a
/// match on the variant pair is enough to skip the change-detection
/// trigger on frames where the user didn't touch vsync.
fn limiters_eq(a: &Limiter, b: &Limiter) -> bool {
    matches!(
        (a, b),
        (Limiter::Auto, Limiter::Auto) | (Limiter::Off, Limiter::Off)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::{PresentMode, WindowMode, WindowResolution};

    #[test]
    fn display_settings_apply_to_primary_window() {
        let mut app = App::new();
        app.insert_resource(ClientSettings::default());
        app.insert_resource(FramepaceSettings {
            limiter: Limiter::Off,
        });
        app.world_mut().spawn((
            PrimaryWindow,
            Window {
                resolution: WindowResolution::new(640, 480),
                present_mode: PresentMode::AutoNoVsync,
                resizable: true,
                ..Default::default()
            },
        ));
        app.add_systems(Update, apply_display_settings_system);

        app.update();

        let window = app
            .world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>()
            .single(app.world())
            .expect("primary window");
        // Always `Immediate`: GPU vsync is handled by `bevy_framepace`
        // on the CPU side, not by the wgpu present mode. See the
        // doc comment on `DisplaySettings::present_mode`.
        assert_eq!(window.present_mode, PresentMode::Immediate);
        assert!(!window.resizable);
        assert_eq!(window.resolution.physical_width(), 1280);
        assert_eq!(window.resolution.physical_height(), 720);

        // Default `vsync: true` should have raised the framepace limiter
        // to `Auto` so the frame rate is capped to the display refresh.
        let limiter = &app.world().resource::<FramepaceSettings>().limiter;
        assert!(matches!(limiter, Limiter::Auto));
    }

    #[test]
    fn leaving_fullscreen_resets_and_centers_windowed_resolution() {
        let mut app = App::new();
        let mut settings = ClientSettings::default();
        settings.display.mode = DisplayMode::Fullscreen;
        settings.display.resolution = DisplayResolution::new(2560, 1440);
        app.insert_resource(settings);
        app.insert_resource(FramepaceSettings {
            limiter: Limiter::Off,
        });
        app.world_mut().spawn((
            PrimaryWindow,
            Window {
                resolution: WindowResolution::new(2560, 1440),
                mode: WindowMode::Fullscreen(
                    MonitorSelection::Primary,
                    bevy::window::VideoModeSelection::Current,
                ),
                ..Default::default()
            },
        ));
        app.add_systems(Update, apply_display_settings_system);

        app.update();
        {
            let mut settings = app.world_mut().resource_mut::<ClientSettings>();
            settings.display.mode = DisplayMode::Windowed;
            settings.display.resolution = DisplayResolution::new(2560, 1440);
        }
        app.update();

        let settings = app.world().resource::<ClientSettings>();
        assert_eq!(settings.display.mode, DisplayMode::Windowed);
        assert_eq!(settings.display.resolution.width, DEFAULT_WINDOWED_WIDTH);
        assert_eq!(settings.display.resolution.height, DEFAULT_WINDOWED_HEIGHT);

        let window = app
            .world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>()
            .single(app.world())
            .expect("primary window");
        assert_eq!(window.mode, WindowMode::Windowed);
        assert_eq!(
            window.position,
            WindowPosition::Centered(MonitorSelection::Primary)
        );
        assert_eq!(window.resolution.physical_width(), DEFAULT_WINDOWED_WIDTH);
        assert_eq!(window.resolution.physical_height(), DEFAULT_WINDOWED_HEIGHT);
    }
}
