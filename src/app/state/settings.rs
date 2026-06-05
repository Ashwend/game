//! Client settings, data, persistence store, and monitor/video-mode
//! helpers. Split into:
//!
//! - `data`, the `ClientSettings` tree and its sanitization rules.
//! - `store`, `ClientSettingsStore` filesystem I/O.
//! - `display`, `display_resolutions`/`best_video_mode` helpers that read
//!   the active `Monitor`.

mod data;
mod display;
mod keybindings;
mod store;

pub(crate) use data::{
    AntiAliasing, ClientSettings, DisplayMode, DisplayResolution, GrassDensity, MAX_FOV_DEG,
    MAX_UI_SCALE, MIN_FOV_DEG, MIN_UI_SCALE, ShadowQuality,
};
pub(crate) use display::display_resolutions;
pub(crate) use keybindings::{KeyAction, KeyBindingCategory, KeyBindingSlot, KeyBindings};
pub(crate) use store::ClientSettingsStore;

#[cfg(test)]
pub(crate) use data::{AudioSettings, DisplaySettings, HudSettings, InputSettings};

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        prelude::*,
        window::{
            Monitor, MonitorSelection, PresentMode, VideoMode, VideoModeSelection, WindowMode,
        },
    };

    fn monitor(video_modes: Vec<VideoMode>) -> Monitor {
        monitor_with_size(1920, 1080, video_modes)
    }

    fn monitor_with_size(width: u32, height: u32, video_modes: Vec<VideoMode>) -> Monitor {
        Monitor {
            name: Some("Display".to_owned()),
            physical_width: width,
            physical_height: height,
            physical_position: IVec2::ZERO,
            refresh_rate_millihertz: Some(60_000),
            scale_factor: 1.0,
            video_modes,
        }
    }

    #[test]
    fn default_settings_match_startup_window() {
        let settings = ClientSettings::default();

        assert_eq!(settings.display.mode, DisplayMode::BorderlessFullscreen);
        assert_eq!(
            settings.display.resolution,
            DisplayResolution::new(1280, 720)
        );
        assert_eq!(settings.display.present_mode(), PresentMode::Immediate);
        assert!(!settings.hud.show_perf_stats);
        // HUD + chat default to visible: a fresh install should show the full
        // interface, the screenshot toggles are opt-in.
        assert!(settings.hud.show_hud);
        assert!(settings.hud.show_chat);
    }

    #[test]
    fn legacy_settings_default_hud_and_chat_visible() {
        // A settings file written before the screenshot toggles existed has no
        // `show_hud` / `show_chat` keys (and may omit the `hud` block entirely).
        // Both must come back `true` so existing players don't boot into a
        // headless HUD after updating.
        let without_keys: ClientSettings =
            serde_json::from_str(r#"{ "hud": { "show_perf_stats": true } }"#)
                .expect("partial hud block should deserialize");
        assert!(without_keys.hud.show_perf_stats);
        assert!(without_keys.hud.show_hud);
        assert!(without_keys.hud.show_chat);

        let without_hud: ClientSettings =
            serde_json::from_str("{}").expect("empty settings should deserialize");
        assert!(without_hud.hud.show_hud);
        assert!(without_hud.hud.show_chat);
    }

    #[test]
    fn settings_store_round_trips_through_sealed_file() {
        let root = std::env::temp_dir().join(format!("game-settings-{}", uuid::Uuid::new_v4()));
        let store = ClientSettingsStore::new(root.join("settings.dat"));
        let mut settings = ClientSettings::default();
        settings.display.mode = DisplayMode::BorderlessFullscreen;
        settings.audio.music_volume = 0.42;
        settings.input.invert_mouse_y = true;

        store.save(&settings).expect("settings should save");
        // The on-disk bytes are sealed, not plain-text JSON: the field names
        // must not appear verbatim.
        let on_disk = std::fs::read(store.path()).expect("settings file written");
        let contains = |needle: &[u8]| on_disk.windows(needle.len()).any(|w| w == needle);
        assert!(
            !contains(b"music_volume"),
            "sealed settings should not contain plain-text JSON"
        );

        let loaded = store.load().expect("settings should load");
        assert_eq!(loaded.display.mode, DisplayMode::BorderlessFullscreen);
        assert_eq!(loaded.audio.music_volume, 0.42);
        assert!(loaded.input.invert_mouse_y);
        assert!(store.path().exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn loaded_settings_are_sanitized() {
        let settings = ClientSettings {
            display: DisplaySettings {
                resolution: DisplayResolution::new(1, 1),
                fov_degrees: 5_000.0,
                ui_scale: 0.0,
                ..Default::default()
            },
            audio: AudioSettings {
                master_volume: 3.0,
                music_volume: 2.0,
                ui_volume: -1.0,
                sfx_volume: 5.0,
            },
            input: InputSettings {
                mouse_sensitivity: 20.0,
                invert_mouse_y: false,
            },
            hud: HudSettings::default(),
            graphics: Default::default(),
            voice: Default::default(),
            onboarding: Default::default(),
            keybindings: Default::default(),
        }
        .sanitized();

        assert_eq!(
            settings.display.resolution,
            DisplayResolution::new(1280, 720)
        );
        assert_eq!(settings.display.fov_degrees, super::data::MAX_FOV_DEG);
        assert_eq!(settings.display.ui_scale, super::data::MIN_UI_SCALE);
        assert_eq!(settings.audio.master_volume, 1.0);
        assert_eq!(settings.audio.music_volume, 1.0);
        assert_eq!(settings.audio.ui_volume, 0.0);
        assert_eq!(settings.audio.sfx_volume, 1.0);
        assert_eq!(settings.input.mouse_sensitivity, 3.0);
    }

    #[test]
    fn non_finite_display_fields_fall_back_to_defaults() {
        let settings = ClientSettings {
            display: DisplaySettings {
                fov_degrees: f32::NAN,
                ui_scale: f32::INFINITY,
                ..Default::default()
            },
            ..Default::default()
        }
        .sanitized();

        assert_eq!(settings.display.fov_degrees, 65.0);
        assert_eq!(settings.display.ui_scale, 1.0);
    }

    #[test]
    fn display_resolutions_use_monitor_modes_when_available() {
        let monitor = monitor(vec![
            VideoMode {
                physical_size: UVec2::new(1920, 1080),
                bit_depth: 24,
                refresh_rate_millihertz: 60_000,
            },
            VideoMode {
                physical_size: UVec2::new(1280, 720),
                bit_depth: 24,
                refresh_rate_millihertz: 60_000,
            },
            VideoMode {
                physical_size: UVec2::new(1920, 1080),
                bit_depth: 24,
                refresh_rate_millihertz: 120_000,
            },
        ]);

        assert_eq!(
            display_resolutions(Some(&monitor), DisplayMode::Windowed),
            vec![
                DisplayResolution::new(1280, 720),
                DisplayResolution::new(1920, 1080),
            ]
        );
    }

    #[test]
    fn exclusive_fullscreen_resolutions_match_monitor_aspect_ratio() {
        let monitor = monitor_with_size(
            5120,
            2880,
            vec![
                VideoMode {
                    physical_size: UVec2::new(5120, 2880),
                    bit_depth: 30,
                    refresh_rate_millihertz: 60_000,
                },
                VideoMode {
                    physical_size: UVec2::new(2560, 1440),
                    bit_depth: 24,
                    refresh_rate_millihertz: 60_000,
                },
                VideoMode {
                    physical_size: UVec2::new(2048, 1080),
                    bit_depth: 24,
                    refresh_rate_millihertz: 60_000,
                },
                VideoMode {
                    physical_size: UVec2::new(1920, 1200),
                    bit_depth: 24,
                    refresh_rate_millihertz: 60_000,
                },
                VideoMode {
                    physical_size: UVec2::new(1920, 1080),
                    bit_depth: 24,
                    refresh_rate_millihertz: 60_000,
                },
            ],
        );

        assert_eq!(
            display_resolutions(Some(&monitor), DisplayMode::Fullscreen),
            vec![
                DisplayResolution::new(1920, 1080),
                DisplayResolution::new(2560, 1440),
                DisplayResolution::new(5120, 2880),
            ]
        );
    }

    #[test]
    fn exclusive_fullscreen_prefers_best_matching_video_mode() {
        let monitor = monitor(vec![
            VideoMode {
                physical_size: UVec2::new(1920, 1080),
                bit_depth: 24,
                refresh_rate_millihertz: 60_000,
            },
            VideoMode {
                physical_size: UVec2::new(1920, 1080),
                bit_depth: 30,
                refresh_rate_millihertz: 120_000,
            },
        ]);
        let settings = DisplaySettings {
            mode: DisplayMode::Fullscreen,
            resolution: DisplayResolution::new(1920, 1080),
            vsync: true,
            ..Default::default()
        };

        assert_eq!(
            settings.window_mode(Some(&monitor)),
            WindowMode::Fullscreen(
                MonitorSelection::Primary,
                VideoModeSelection::Specific(VideoMode {
                    physical_size: UVec2::new(1920, 1080),
                    bit_depth: 30,
                    refresh_rate_millihertz: 120_000,
                })
            )
        );
    }
}
