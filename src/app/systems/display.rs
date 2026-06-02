use bevy::{
    prelude::*,
    window::{Monitor, MonitorSelection, PrimaryMonitor, PrimaryWindow, Window, WindowPosition},
};
use bevy_framepace::{FramepaceSettings, Limiter};

use super::super::state::{ClientSettings, DisplayMode, DisplayResolution};

const DEFAULT_WINDOWED_WIDTH: u32 = 1280;
const DEFAULT_WINDOWED_HEIGHT: u32 = 720;

/// Smallest window **logical** inner size the UI is laid out for. The main menu
/// (a 360 pt panel under a 78 pt title) and the in-game overlays need roughly
/// this much room; below it the egui viewport is too short for the menu to fit
/// and it overflows off-screen entirely, the player sees only the 3D backdrop.
///
/// This is in *logical points*, not physical pixels, on purpose. The player's
/// resolution setting is a physical-pixel count, but on a HiDPI display (e.g. a
/// Retina Mac, scale factor 2) a 1280x720 *physical* window is only 640x360
/// *points*, too small for the menu. Flooring the *logical* size fixes it
/// regardless of the display's scale factor.
///
/// The height matches the default windowed resolution's height (720) because
/// the menu and overlays were laid out against that; anything shorter risks
/// clipping the menu. The cap to the monitor's physical size still applies, so
/// genuinely small screens just get the largest window that fits.
const MIN_WINDOW_LOGICAL_WIDTH: f32 = 1000.0;
const MIN_WINDOW_LOGICAL_HEIGHT: f32 = 720.0;

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
        let scale = window.resolution.scale_factor();
        let monitor_physical =
            primary_monitor.map(|monitor| (monitor.physical_width, monitor.physical_height));
        let (target_width, target_height) =
            windowed_physical_target(settings.display.resolution, scale, monitor_physical);
        if window.resolution.physical_width() != target_width
            || window.resolution.physical_height() != target_height
        {
            window
                .resolution
                .set_physical_resolution(target_width, target_height);
        }
    }
}

/// Physical inner size to request for the windowed primary window.
///
/// Starts from the player's chosen physical [`DisplayResolution`], raises it so
/// the resulting *logical* size is at least
/// [`MIN_WINDOW_LOGICAL_WIDTH`]x[`MIN_WINDOW_LOGICAL_HEIGHT`] (so the menu always
/// fits, see those constants), then caps it to the monitor's physical size so
/// we never request a window larger than the screen. `monitor_physical` is
/// `None` only on the first frames before the primary monitor is known; the cap
/// is skipped until it arrives and the next frame re-runs with it.
fn windowed_physical_target(
    requested: DisplayResolution,
    scale_factor: f32,
    monitor_physical: Option<(u32, u32)>,
) -> (u32, u32) {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let min_width = (MIN_WINDOW_LOGICAL_WIDTH * scale).ceil() as u32;
    let min_height = (MIN_WINDOW_LOGICAL_HEIGHT * scale).ceil() as u32;
    let (cap_width, cap_height) = monitor_physical.unwrap_or((u32::MAX, u32::MAX));
    (
        requested.width.max(min_width).min(cap_width.max(1)),
        requested.height.max(min_height).min(cap_height.max(1)),
    )
}

/// `Limiter` doesn't derive `PartialEq` in `bevy_framepace`. We only
/// ever swap between `Auto` and `Off`, `Manual` isn't used, so a
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
    fn windowed_floor_raises_tiny_logical_size_on_hidpi() {
        // 1280x720 physical on a Retina (scale 2) display is only 640x360
        // points, too small for the menu. Floor it to 1000x720 logical, i.e.
        // 2000x1440 physical.
        let target =
            windowed_physical_target(DisplayResolution::new(1280, 720), 2.0, Some((5120, 2880)));
        assert_eq!(target, (2000, 1440));
    }

    #[test]
    fn windowed_floor_is_noop_at_scale_one_above_minimum() {
        // On a standard-DPI display the chosen resolution already clears the
        // logical floor, so it passes through unchanged.
        let target =
            windowed_physical_target(DisplayResolution::new(1280, 720), 1.0, Some((2560, 1440)));
        assert_eq!(target, (1280, 720));
    }

    #[test]
    fn windowed_target_is_capped_to_monitor() {
        // A windowed resolution larger than the screen is clamped down so the
        // window never exceeds the monitor.
        let target =
            windowed_physical_target(DisplayResolution::new(3840, 2160), 1.0, Some((1366, 768)));
        assert_eq!(target, (1366, 768));
    }

    #[test]
    fn windowed_target_prefers_monitor_when_floor_exceeds_it() {
        // If the logical floor would need a window bigger than the monitor,
        // fall back to the full monitor size rather than overflowing it.
        let target =
            windowed_physical_target(DisplayResolution::new(1280, 720), 2.0, Some((1280, 800)));
        assert_eq!(target, (1280, 800));
    }

    #[test]
    fn windowed_target_treats_invalid_scale_as_one() {
        let target = windowed_physical_target(DisplayResolution::new(640, 480), f32::NAN, None);
        // min floor at scale 1 is 1000x720; requested is smaller, so it rises.
        assert_eq!(target, (1000, 720));
    }

    #[test]
    fn display_settings_apply_to_primary_window() {
        let mut app = App::new();
        // This case exercises the windowed-resolution path, so pin the mode
        // to Windowed rather than relying on the (now borderless) default.
        let mut settings = ClientSettings::default();
        settings.display.mode = DisplayMode::Windowed;
        app.insert_resource(settings);
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
