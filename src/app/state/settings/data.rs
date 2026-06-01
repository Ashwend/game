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
    pub(crate) graphics: GraphicsSettings,
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
            mode: DisplayMode::BorderlessFullscreen,
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
    Windowed,
    /// Default for a fresh install: fills the primary monitor with no window
    /// chrome. A persisted settings file overrides this with the player's
    /// saved choice (the `mode` field is always written on save).
    #[default]
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

/// Rendering options the player can dial in from the Graphics tab. These are
/// all **client-side visual** knobs — none of them affect gameplay or what the
/// server simulates, so they're free to differ between players. HDR is *not*
/// exposed here: it's a required baseline for the procedural atmosphere sky and
/// is always on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GraphicsSettings {
    /// Whether the camera runs the bloom (light-glow) post-process pass. The
    /// strength is intentionally not exposed — it's fixed at Bevy's natural
    /// preset; players just get a simple on/off like most games offer.
    #[serde(default = "default_bloom_enabled")]
    pub(crate) bloom_enabled: bool,
    /// In-game anti-aliasing mode. Defaults to FXAA: MSAA composites badly with
    /// the procedural atmosphere sky (dark, shimmering fringes where geometry
    /// meets the fullscreen sky pass), so FXAA is the clean default with MSAA
    /// left as an explicit choice. The menu backdrop ignores this (it leans on
    /// depth-of-field instead).
    #[serde(default)]
    pub(crate) anti_aliasing: AntiAliasing,
    /// Sun shadow quality. Re-rendering every tree into the shadow cascades is
    /// a major GPU cost in dense forest, so this is a real perf lever.
    #[serde(default)]
    pub(crate) shadows: ShadowQuality,
    /// Density of the procedural detail grass — a client-only cosmetic ground
    /// layer streamed in tiles around the camera. Higher tiers raise the blade
    /// count and the draw radius (more GPU cost), `Off` removes it entirely.
    #[serde(default)]
    pub(crate) grass_density: GrassDensity,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            bloom_enabled: default_bloom_enabled(),
            anti_aliasing: AntiAliasing::default(),
            shadows: ShadowQuality::default(),
            grass_density: GrassDensity::default(),
        }
    }
}

/// Procedural detail-grass density. Cosmetic, client-side, seed-free — none of
/// it touches gameplay, collision, or the server, so it's free to differ
/// between players. `Off` despawns all grass; the other tiers map to a
/// blades-per-tile + draw-radius pair in the grass renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GrassDensity {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl GrassDensity {
    pub(crate) const ALL: [Self; 4] = [Self::Off, Self::Low, Self::Medium, Self::High];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

/// Resolved sun-shadow parameters for a [`ShadowQuality`] level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ShadowConfig {
    /// Far bound of the shadow cascades, in metres. Smaller = fewer trees
    /// re-rendered into the cascades.
    pub(crate) maximum_distance: f32,
    pub(crate) num_cascades: usize,
    /// Per-cascade shadow map resolution (power of two).
    pub(crate) map_size: usize,
}

/// Sun shadow quality. Shadows over a dense forest re-render every tree into
/// each cascade, which is one of the heaviest GPU costs, so this trades shadow
/// distance / cascade count / map resolution against framerate. `Off` disables
/// sun shadows entirely. `High` matches the engine defaults the scene shipped
/// with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ShadowQuality {
    Off,
    Low,
    #[default]
    High,
}

impl ShadowQuality {
    pub(crate) const ALL: [Self; 3] = [Self::Off, Self::Low, Self::High];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::High => "High",
        }
    }

    /// Resolved cascade/map config, or `None` when sun shadows are disabled.
    pub(crate) fn config(self) -> Option<ShadowConfig> {
        match self {
            Self::Off => None,
            Self::Low => Some(ShadowConfig {
                maximum_distance: 45.0,
                num_cascades: 2,
                map_size: 1024,
            }),
            Self::High => Some(ShadowConfig {
                maximum_distance: 100.0,
                num_cascades: 3,
                map_size: 2048,
            }),
        }
    }
}

/// In-game anti-aliasing mode. FXAA is the default because MSAA leaves dark,
/// shimmering fringes where geometry meets the procedural atmosphere sky (the
/// fullscreen sky pass doesn't resolve cleanly under multisampling — Bevy's own
/// atmosphere example uses FXAA for the same reason). MSAA is still offered for
/// players who prefer its sharper interior edges and don't mind the fringing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AntiAliasing {
    Off,
    #[default]
    Fxaa,
    Msaa2,
    Msaa4,
}

impl AntiAliasing {
    pub(crate) const ALL: [Self; 4] = [Self::Off, Self::Fxaa, Self::Msaa2, Self::Msaa4];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Fxaa => "FXAA",
            Self::Msaa2 => "MSAA 2x",
            Self::Msaa4 => "MSAA 4x",
        }
    }

    /// The Bevy MSAA sample count for this mode (`Off` for the FXAA/Off modes).
    pub(crate) fn msaa(self) -> Msaa {
        match self {
            Self::Msaa2 => Msaa::Sample2,
            Self::Msaa4 => Msaa::Sample4,
            Self::Off | Self::Fxaa => Msaa::Off,
        }
    }

    /// Whether the FXAA post-process pass should run on the camera.
    pub(crate) fn fxaa_enabled(self) -> bool {
        matches!(self, Self::Fxaa)
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

fn default_bloom_enabled() -> bool {
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
