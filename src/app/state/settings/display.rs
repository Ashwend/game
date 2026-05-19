use bevy::window::{Monitor, VideoMode};

use super::data::{DisplayMode, DisplayResolution};

const FALLBACK_RESOLUTIONS: [DisplayResolution; 6] = [
    DisplayResolution::new(1280, 720),
    DisplayResolution::new(1366, 768),
    DisplayResolution::new(1600, 900),
    DisplayResolution::new(1920, 1080),
    DisplayResolution::new(2560, 1440),
    DisplayResolution::new(3840, 2160),
];

pub(crate) fn display_resolutions(
    monitor: Option<&Monitor>,
    display_mode: DisplayMode,
) -> Vec<DisplayResolution> {
    let mut resolutions = monitor
        .map(|monitor| {
            monitor
                .video_modes
                .iter()
                .map(video_mode_resolution)
                .filter(|resolution| {
                    display_mode != DisplayMode::Fullscreen
                        || resolution_matches_monitor_aspect(monitor, *resolution)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if resolutions.is_empty() {
        resolutions.extend(FALLBACK_RESOLUTIONS);
        if let Some(monitor) = monitor {
            resolutions.push(DisplayResolution::new(
                monitor.physical_width,
                monitor.physical_height,
            ));
            if display_mode == DisplayMode::Fullscreen {
                resolutions
                    .retain(|resolution| resolution_matches_monitor_aspect(monitor, *resolution));
            }
        }
    }

    resolutions.sort_by_key(|resolution| {
        (
            u64::from(resolution.width) * u64::from(resolution.height),
            resolution.width,
            resolution.height,
        )
    });
    resolutions.dedup();
    resolutions
}

pub(super) fn best_video_mode(
    monitor: Option<&Monitor>,
    resolution: DisplayResolution,
) -> Option<VideoMode> {
    let monitor = monitor?;
    if !resolution_matches_monitor_aspect(monitor, resolution) {
        return None;
    }

    monitor
        .video_modes
        .iter()
        .copied()
        .filter(|mode| video_mode_resolution(mode) == resolution)
        .max_by_key(|mode| (mode.refresh_rate_millihertz, mode.bit_depth))
}

fn video_mode_resolution(mode: &VideoMode) -> DisplayResolution {
    DisplayResolution::new(mode.physical_size.x, mode.physical_size.y)
}

fn resolution_matches_monitor_aspect(monitor: &Monitor, resolution: DisplayResolution) -> bool {
    if monitor.physical_width == 0 || monitor.physical_height == 0 || resolution.height == 0 {
        return false;
    }

    let monitor_aspect = monitor.physical_width as f32 / monitor.physical_height as f32;
    let resolution_aspect = resolution.width as f32 / resolution.height as f32;
    (monitor_aspect - resolution_aspect).abs() <= 0.01
}
