//! In-game voice chat. Three concerns split across submodules:
//!
//! - [`codec`] — thin libopus wrapper.
//! - [`capture`] — microphone input + opus encode worker.
//! - [`playback`] — opus decode + per-speaker spatial mixer + output stream.
//! - [`systems`] — Bevy resources/systems that bridge the worker threads
//!   to the protocol and to the UI indicator.

pub(crate) mod capture;
pub(crate) mod codec;
pub(crate) mod playback;
pub(crate) mod systems;

pub(crate) use systems::{
    IncomingVoiceMessage, VoiceState, apply_voice_settings_system, manage_voice_capture_system,
    receive_voice_system, setup_voice_system, transmit_voice_system,
};
