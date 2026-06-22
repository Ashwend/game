//! cpal device enumeration + select-by-name, shared by the capture/playback
//! worker threads and the options device picker.
//!
//! The picker stores a device by its cpal **name** (a `String`), not a live
//! handle, because (a) settings persist to disk across runs and (b) a
//! `cpal::Device` is `!Send` on macOS so it can't be passed to the worker
//! thread anyway. Resolution happens on the worker thread: we re-enumerate and
//! match by name, falling back to the system default when the saved device is
//! gone (a headset was unplugged, a virtual device was removed). That fallback
//! is the whole reason we re-resolve instead of caching a handle: the player
//! should keep hearing/being-heard on *some* device rather than going silent
//! because their preferred one vanished.

use cpal::traits::{DeviceTrait, HostTrait};

/// All input device names the host can see, in enumeration order. Best-effort:
/// returns an empty list if cpal can't enumerate (rare; logged by the caller).
pub(crate) fn list_input_device_names(host: &cpal::Host) -> Vec<String> {
    match host.input_devices() {
        Ok(devices) => devices.filter_map(|device| device.name().ok()).collect(),
        Err(error) => {
            bevy::log::warn!("voice: listing input devices failed: {error}");
            Vec::new()
        }
    }
}

/// All output device names the host can see, in enumeration order.
pub(crate) fn list_output_device_names(host: &cpal::Host) -> Vec<String> {
    match host.output_devices() {
        Ok(devices) => devices.filter_map(|device| device.name().ok()).collect(),
        Err(error) => {
            bevy::log::warn!("voice: listing output devices failed: {error}");
            Vec::new()
        }
    }
}

/// Resolve the input device to open: the named one if it still exists,
/// otherwise the system default. Returns `None` only when there is no input
/// device at all.
pub(crate) fn resolve_input_device(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    if let Some(wanted) = name
        && let Some(found) = find_device(host.input_devices().ok(), wanted)
    {
        return Some(found);
    }
    if let Some(wanted) = name {
        bevy::log::warn!("voice: input device {wanted:?} not found; using system default");
    }
    host.default_input_device()
}

/// Resolve the output device to open: the named one if it still exists,
/// otherwise the system default.
pub(crate) fn resolve_output_device(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    if let Some(wanted) = name
        && let Some(found) = find_device(host.output_devices().ok(), wanted)
    {
        return Some(found);
    }
    if let Some(wanted) = name {
        bevy::log::warn!("voice: output device {wanted:?} not found; using system default");
    }
    host.default_output_device()
}

fn find_device<I>(devices: Option<I>, wanted: &str) -> Option<cpal::Device>
where
    I: Iterator<Item = cpal::Device>,
{
    devices?.find(|device| device.name().is_ok_and(|name| name == wanted))
}
