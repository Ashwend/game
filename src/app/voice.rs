//! In-game voice chat. Concerns split across submodules:
//!
//! - [`codec`], thin libopus wrapper.
//! - [`capture`], microphone input + opus encode worker.
//! - [`playback`], opus decode + per-speaker spatial mixer + output stream.
//! - [`resample`], shared linear resampler used by both capture (any rate to
//!   48 kHz) and playback (48 kHz to the output device rate).
//! - [`devices`], cpal device enumeration + select-by-name helpers shared by
//!   capture and playback so the options device picker and the worker threads
//!   agree on names.
//! - [`systems`], Bevy resources/systems that bridge the worker threads
//!   to the protocol and to the UI indicator.

pub(crate) mod capture;
pub(crate) mod codec;
pub(crate) mod devices;
pub(crate) mod playback;
pub(crate) mod resample;
pub(crate) mod systems;

pub(crate) use systems::{
    IncomingVoiceMessage, VoiceDeviceCache, VoiceDisabled, VoiceState, VoiceUiControl,
    apply_voice_settings_system, manage_voice_capture_system, manage_voice_monitor_system,
    manage_voice_playback_system, receive_voice_system, refresh_voice_devices_system,
    setup_voice_system, transmit_voice_system,
};
