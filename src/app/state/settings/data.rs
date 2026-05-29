use bevy::{
    prelude::*,
    window::{Monitor, MonitorSelection, PresentMode, VideoModeSelection, WindowMode},
};
use bevy_framepace::Limiter;
use serde::{Deserialize, Serialize};

use super::{display::best_video_mode, keybindings::KeyBindings};

#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ClientSettings {
    #[serde(default)]
    pub(crate) display: DisplaySettings,
    #[serde(default)]
    pub(crate) audio: AudioSettings,
    #[serde(default)]
    pub(crate) voice: VoiceSettings,
    #[serde(default)]
    pub(crate) input: InputSettings,
    #[serde(default)]
    pub(crate) hud: HudSettings,
    #[serde(default)]
    pub(crate) keybindings: KeyBindings,
    #[serde(default)]
    pub(crate) identity: IdentitySettings,
}

/// Stable per-installation identity. Persisted alongside the other settings so
/// the same machine keeps the same player (and saved inventory) across
/// launches, and so two installs no longer collide on the single hardcoded
/// offline id. A real Steam login would supersede this in the future.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IdentitySettings {
    /// Offline-auth Steam id for this installation. `0` means "not generated
    /// yet"; the client resolves and persists a real value on first launch
    /// (see `crate::steam::generate_install_id`).
    #[serde(default)]
    pub(crate) install_id: u64,
}

impl ClientSettings {
    pub(crate) fn sanitized(mut self) -> Self {
        self.display = self.display.sanitized();
        self.audio.master_volume = self.audio.master_volume.clamp(0.0, 1.0);
        self.audio.music_volume = self.audio.music_volume.clamp(0.0, 1.0);
        self.audio.ui_volume = self.audio.ui_volume.clamp(0.0, 1.0);
        self.audio.sfx_volume = self.audio.sfx_volume.clamp(0.0, 1.0);
        self.voice = self.voice.sanitized();
        self.input.mouse_sensitivity = self.input.mouse_sensitivity.clamp(0.25, 3.0);
        self.keybindings = self.keybindings.sanitized();
        self
    }
}

/// Minimum/maximum vertical field of view the player can dial in, in degrees.
/// The lower bound keeps the view from collapsing into a telescope; the upper
/// bound stops the fish-eye distortion that makes the world hard to read. The
/// run-speed FOV boost stacks on top of whatever the player picks. The default
/// matches the camera's historical baked-in baseline so existing saves look
/// identical until the player changes it.
pub(crate) const MIN_FOV_DEG: f32 = 50.0;
pub(crate) const MAX_FOV_DEG: f32 = 100.0;

/// Minimum/maximum egui UI scale (pixels-per-point multiplier). 1.0 is the
/// platform default; below 0.75 text becomes unreadable on most displays and
/// above 1.5 the chrome starts clipping the bounded panels.
pub(crate) const MIN_UI_SCALE: f32 = 0.75;
pub(crate) const MAX_UI_SCALE: f32 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct DisplaySettings {
    #[serde(default)]
    pub(crate) mode: DisplayMode,
    #[serde(default = "default_resolution")]
    pub(crate) resolution: DisplayResolution,
    #[serde(default = "default_vsync")]
    pub(crate) vsync: bool,
    /// Base horizontal field of view in degrees. See [`MIN_FOV_DEG`].
    #[serde(default = "default_fov")]
    pub(crate) fov_degrees: f32,
    /// egui pixels-per-point multiplier. See [`MIN_UI_SCALE`].
    #[serde(default = "default_ui_scale")]
    pub(crate) ui_scale: f32,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            mode: DisplayMode::Windowed,
            resolution: DisplayResolution::new(1280, 720),
            vsync: true,
            fov_degrees: default_fov(),
            ui_scale: default_ui_scale(),
        }
    }
}

impl DisplaySettings {
    /// The wgpu present mode for the primary window.
    ///
    /// Always `Immediate` regardless of the user's vsync preference: GPU
    /// vsync (`Fifo`/`AutoVsync`) misbehaves on macOS Metal — `Fifo`
    /// flickers, `AutoVsync` fails to cap the frame rate at all. Frame
    /// limiting is handled CPU-side by `bevy_framepace`, which works
    /// reliably across platforms. See [`Self::frame_limiter`].
    pub(crate) fn present_mode(self) -> PresentMode {
        PresentMode::Immediate
    }

    /// The CPU-side frame limiter applied by `bevy_framepace`.
    ///
    /// `vsync: true` caps the frame rate to the display's refresh by
    /// putting the main thread to sleep just before the next frame is
    /// presented. `vsync: false` runs uncapped (tearing is possible but
    /// frames are still individually fast).
    pub(crate) fn frame_limiter(self) -> Limiter {
        if self.vsync {
            Limiter::Auto
        } else {
            Limiter::Off
        }
    }

    pub(super) fn sanitized(mut self) -> Self {
        self.resolution = self.resolution.sanitized();
        self.fov_degrees = if self.fov_degrees.is_finite() {
            self.fov_degrees.clamp(MIN_FOV_DEG, MAX_FOV_DEG)
        } else {
            default_fov()
        };
        self.ui_scale = if self.ui_scale.is_finite() {
            self.ui_scale.clamp(MIN_UI_SCALE, MAX_UI_SCALE)
        } else {
            default_ui_scale()
        };
        self
    }

    pub(crate) fn window_mode(self, monitor: Option<&Monitor>) -> WindowMode {
        match self.mode {
            DisplayMode::Windowed => WindowMode::Windowed,
            DisplayMode::BorderlessFullscreen => {
                WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
            }
            DisplayMode::Fullscreen => WindowMode::Fullscreen(
                MonitorSelection::Primary,
                best_video_mode(monitor, self.resolution)
                    .map(VideoModeSelection::Specific)
                    .unwrap_or(VideoModeSelection::Current),
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DisplayMode {
    #[default]
    Windowed,
    BorderlessFullscreen,
    Fullscreen,
}

impl DisplayMode {
    pub(crate) const ALL: [Self; 3] =
        [Self::Windowed, Self::BorderlessFullscreen, Self::Fullscreen];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Windowed => "Windowed",
            Self::BorderlessFullscreen => "Borderless Fullscreen",
            Self::Fullscreen => "Fullscreen",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct DisplayResolution {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl DisplayResolution {
    pub(crate) const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub(crate) fn label(self) -> String {
        format!("{} x {}", self.width, self.height)
    }

    pub(super) fn sanitized(self) -> Self {
        if self.width < 640 || self.height < 360 {
            default_resolution()
        } else {
            self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct AudioSettings {
    /// Overall mix gain applied on top of every per-category slider. Lets the
    /// player drop the whole game volume without losing their relative
    /// music/effects/interface balance.
    #[serde(default = "default_volume")]
    pub(crate) master_volume: f32,
    #[serde(default = "default_volume")]
    pub(crate) music_volume: f32,
    #[serde(default = "default_volume")]
    pub(crate) ui_volume: f32,
    #[serde(default = "default_volume")]
    pub(crate) sfx_volume: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            master_volume: 1.0,
            music_volume: 1.0,
            ui_volume: 1.0,
            sfx_volume: 1.0,
        }
    }
}

/// Voice-chat tuning the player can dial in from the options panel. Stored on
/// disk alongside the other [`ClientSettings`] tabs.
///
/// Note: the *audible distance* is intentionally NOT a setting. It's a core
/// gameplay rule — how far your voice carries is part of the game design,
/// not a personal preference — and lives as a constant on the server-side
/// voice module so both halves of the system agree.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct VoiceSettings {
    /// Whether voice transmit is allowed at all. Disabling here shuts off the
    /// microphone capture thread on the client.
    #[serde(default = "default_voice_enabled")]
    pub(crate) enabled: bool,
    /// Output gain applied to every incoming voice stream. `0.0` is silent,
    /// `1.0` is unity gain. The per-stream spatial gain is computed at mix
    /// time and multiplied on top of this.
    #[serde(default = "default_volume")]
    pub(crate) output_volume: f32,
    /// Input gain applied to the microphone before encoding.
    #[serde(default = "default_volume")]
    pub(crate) input_volume: f32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            enabled: default_voice_enabled(),
            output_volume: default_volume(),
            input_volume: default_volume(),
        }
    }
}

impl VoiceSettings {
    fn sanitized(mut self) -> Self {
        self.output_volume = self.output_volume.clamp(0.0, 1.0);
        self.input_volume = self.input_volume.clamp(0.0, 1.0);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct InputSettings {
    #[serde(default = "default_mouse_sensitivity")]
    pub(crate) mouse_sensitivity: f32,
    #[serde(default)]
    pub(crate) invert_mouse_y: bool,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 1.0,
            invert_mouse_y: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HudSettings {
    /// Toggles the perf overlay (FPS, chunk position, loaded chunks, live
    /// nodes, regrow queue, AoI count). Bound to F2 in-game.
    #[serde(default)]
    pub(crate) show_perf_stats: bool,
    /// Debug overlay that draws the 64 m world-chunk boundaries around
    /// the player as vertical fading walls. Useful when diagnosing AoI
    /// streaming behaviour or boundary-crossing glitches.
    #[serde(default)]
    pub(crate) show_chunk_overlay: bool,
    /// AoI view tier sent to the server. Low/Medium/High map to a
    /// concentric Chebyshev ring of 1/2/3 chunks around the player.
    #[serde(default)]
    pub(crate) view_radius: crate::protocol::ViewRadiusTier,
}

pub(super) fn default_resolution() -> DisplayResolution {
    DisplayResolution::new(1280, 720)
}

fn default_vsync() -> bool {
    true
}

fn default_fov() -> f32 {
    65.0
}

fn default_ui_scale() -> f32 {
    1.0
}

fn default_volume() -> f32 {
    1.0
}

fn default_mouse_sensitivity() -> f32 {
    1.0
}

fn default_voice_enabled() -> bool {
    true
}
