use bevy::{
    prelude::*,
    window::{Monitor, MonitorSelection, PresentMode, VideoModeSelection, WindowMode},
};
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
}

impl ClientSettings {
    pub(crate) fn sanitized(mut self) -> Self {
        self.display.resolution = self.display.resolution.sanitized();
        self.audio.music_volume = self.audio.music_volume.clamp(0.0, 1.0);
        self.audio.ui_volume = self.audio.ui_volume.clamp(0.0, 1.0);
        self.audio.sfx_volume = self.audio.sfx_volume.clamp(0.0, 1.0);
        self.voice = self.voice.sanitized();
        self.input.mouse_sensitivity = self.input.mouse_sensitivity.clamp(0.25, 3.0);
        self.keybindings = self.keybindings.sanitized();
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DisplaySettings {
    #[serde(default)]
    pub(crate) mode: DisplayMode,
    #[serde(default = "default_resolution")]
    pub(crate) resolution: DisplayResolution,
    #[serde(default = "default_vsync")]
    pub(crate) vsync: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            mode: DisplayMode::Windowed,
            resolution: DisplayResolution::new(1280, 720),
            vsync: true,
        }
    }
}

impl DisplaySettings {
    pub(crate) fn present_mode(self) -> PresentMode {
        if self.vsync {
            PresentMode::AutoVsync
        } else {
            PresentMode::AutoNoVsync
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HudSettings {
    #[serde(default = "default_show_fps")]
    pub(crate) show_fps: bool,
}

impl Default for HudSettings {
    fn default() -> Self {
        Self { show_fps: true }
    }
}

pub(super) fn default_resolution() -> DisplayResolution {
    DisplayResolution::new(1280, 720)
}

fn default_vsync() -> bool {
    true
}

fn default_volume() -> f32 {
    1.0
}

fn default_mouse_sensitivity() -> f32 {
    1.0
}

fn default_show_fps() -> bool {
    true
}

fn default_voice_enabled() -> bool {
    true
}
