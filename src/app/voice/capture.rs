//! Microphone capture + Opus encode pipeline.
//!
//! cpal's `Stream` is `!Send` on macOS, so we keep it in a dedicated worker
//! thread and talk to it over crossbeam channels. The thread owns the
//! encoder; the main thread drains encoded frames out of `frames_rx` once
//! per `Update` and only ships them on the wire while push-to-talk is held.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow};
use cpal::{
    SampleFormat, StreamConfig,
    traits::{DeviceTrait, StreamTrait},
};
use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};

use crate::protocol::{VOICE_FRAME_SAMPLES, VOICE_SAMPLE_RATE_HZ};

use super::codec::VoiceEncoder;
use super::devices::resolve_input_device;
use super::resample::LinearResampler;

enum CaptureCmd {
    Shutdown,
}

/// Terminal outcome of the worker thread's mic initialisation, surfaced to the
/// main thread by [`VoiceCapture::poll_status`] without ever blocking it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureStatus {
    /// The cpal input stream is open and shipping frames.
    Ready,
    /// No device matched, cpal refused, or the OS denied microphone access.
    Failed,
}

/// Owns the cpal mic stream and ships encoded Opus frames out via
/// `frames_rx`. Spawned once when the voice subsystem starts and torn down
/// only when the app exits.
pub(crate) struct VoiceCapture {
    pub(crate) frames_rx: Receiver<Vec<u8>>,
    /// One-shot readiness signal from the worker: `Ok` once the stream is
    /// live, `Err` if init failed. Polled non-blocking by [`Self::poll_status`].
    ready_rx: Receiver<Result<(), String>>,
    cmd_tx: Sender<CaptureCmd>,
    /// Mic gain as a fixed-point f32 (input_gain * 1_000_000). Atomic so the
    /// audio thread can read it on every callback without a lock.
    gain_micro: Arc<AtomicU32>,
    /// Most recent post-gain peak amplitude as a fixed-point f32, written by
    /// the audio callback and read by the options mic-test meter. Atomic for
    /// the same lock-free reason as `gain_micro`.
    level_micro: Arc<AtomicU32>,
    worker: Option<JoinHandle<()>>,
    /// Cached terminal status. `None` while the worker is still initialising
    /// (which on macOS includes the time the OS mic-permission dialog is up).
    status: Option<CaptureStatus>,
}

impl VoiceCapture {
    /// Opens the microphone. `device_name` selects a specific input device by
    /// its cpal name; `None` (or a name that no longer matches, e.g. an
    /// unplugged headset) uses the system default.
    pub(crate) fn spawn(device_name: Option<String>) -> Result<Self> {
        let (frames_tx, frames_rx) = unbounded::<Vec<u8>>();
        let (cmd_tx, cmd_rx) = unbounded::<CaptureCmd>();
        let (ready_tx, ready_rx) = unbounded::<Result<(), String>>();
        let gain_micro = Arc::new(AtomicU32::new(linear_to_micro(1.0)));
        let level_micro = Arc::new(AtomicU32::new(0));
        let gain_for_thread = Arc::clone(&gain_micro);
        let level_for_thread = Arc::clone(&level_micro);

        let worker = thread::Builder::new()
            .name("voice-capture".into())
            .spawn(move || {
                if let Err(error) = run_capture(
                    device_name,
                    frames_tx,
                    cmd_rx,
                    ready_tx,
                    gain_for_thread,
                    level_for_thread,
                ) {
                    bevy::log::warn!("voice capture stopped: {error:#}");
                }
            })
            .context("failed to spawn voice capture thread")?;

        // Deliberately do NOT block on the worker's readiness here. Opening the
        // cpal input stream raises the OS microphone-permission prompt on
        // macOS, and the player can take several seconds to answer it. Blocking
        // the main thread that long stalls the whole Bevy schedule, including
        // the Lightyear network tick, so a connection that just completed its
        // handshake gets dropped on a missed keepalive. We hand the handle back
        // immediately and let `poll_status` observe the outcome over the next
        // frames; `voice.available` only flips on once the stream is actually
        // live.
        Ok(Self {
            frames_rx,
            ready_rx,
            cmd_tx,
            gain_micro,
            level_micro,
            worker: Some(worker),
            status: None,
        })
    }

    pub(crate) fn set_input_gain(&self, gain: f32) {
        self.gain_micro
            .store(linear_to_micro(gain.clamp(0.0, 1.0)), Ordering::Relaxed);
    }

    /// Most recent post-gain peak amplitude (0..~1) seen by the audio
    /// callback. Drives the options "Test Microphone" level meter; the UI
    /// smooths it for display. Reads 0 before the first callback or if the
    /// stream failed.
    pub(crate) fn input_level(&self) -> f32 {
        micro_to_linear(self.level_micro.load(Ordering::Relaxed))
    }

    /// Non-blocking. Returns the worker's terminal status exactly once, on the
    /// frame it resolves; `None` both before resolution ("still initialising")
    /// and on every call after ("no change", act on the prior status). The
    /// detailed failure reason is already logged by the worker, so the `Err`
    /// payload is consumed silently here.
    pub(crate) fn poll_status(&mut self) -> Option<CaptureStatus> {
        if self.status.is_some() {
            return None;
        }
        let resolved = match self.ready_rx.try_recv() {
            Ok(Ok(())) => CaptureStatus::Ready,
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => CaptureStatus::Failed,
            Err(TryRecvError::Empty) => return None,
        };
        self.status = Some(resolved);
        Some(resolved)
    }

    /// `true` once the cpal input stream is open and shipping frames. False
    /// while the permission prompt is up or if init failed, so the PTT
    /// indicator doesn't claim "transmitting" before the mic is actually hot.
    pub(crate) fn is_ready(&self) -> bool {
        self.status == Some(CaptureStatus::Ready)
    }
}

impl Drop for VoiceCapture {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(CaptureCmd::Shutdown);
        let Some(handle) = self.worker.take() else {
            return;
        };
        // Only join once the worker has resolved. If it's still initialising it
        // may be parked inside the OS permission dialog; joining there would
        // re-stall the main thread for as long as the dialog stays up, the very
        // hang we moved off the main thread to begin with. We've signalled
        // shutdown (and dropping `cmd_tx` also wakes the worker), so it tears
        // its own stream down once the prompt resolves and then exits. The cpal
        // stream is owned entirely by that thread, so detaching is safe.
        if self.status.is_some() {
            let _ = handle.join();
        }
    }
}

fn run_capture(
    device_name: Option<String>,
    frames_tx: Sender<Vec<u8>>,
    cmd_rx: Receiver<CaptureCmd>,
    ready_tx: Sender<Result<(), String>>,
    gain_micro: Arc<AtomicU32>,
    level_micro: Arc<AtomicU32>,
) -> Result<()> {
    let host = cpal::default_host();
    let Some(device) = resolve_input_device(&host, device_name.as_deref()) else {
        let _ = ready_tx.send(Err("no audio input device".to_owned()));
        return Ok(());
    };
    let supported = match find_supported_config(&device) {
        Ok(config) => config,
        Err(error) => {
            let msg = format!("voice capture disabled: {error:#}");
            bevy::log::warn!("{msg}");
            let _ = ready_tx.send(Err(msg));
            return Ok(());
        }
    };
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let input_channels = config.channels as usize;
    let input_rate = config.sample_rate.0;
    bevy::log::info!(
        "voice capture: device={:?} format={:?} rate={} channels={}",
        device.name().ok(),
        sample_format,
        input_rate,
        input_channels
    );
    let mut encoder = VoiceEncoder::new()?;
    let mut accumulator: Vec<f32> = Vec::with_capacity(VOICE_FRAME_SAMPLES * 2);
    let mut resampler = LinearResampler::new(input_rate, VOICE_SAMPLE_RATE_HZ);

    let build_result = build_stream(
        &device,
        sample_format,
        &config,
        move |interleaved: &[f32]| {
            let gain = micro_to_linear(gain_micro.load(Ordering::Relaxed));
            let mono = downmix_to_mono(interleaved, input_channels, gain);
            // Publish the post-gain peak for the options mic-test meter. Cheap
            // (one pass, no alloc) and lock-free; the UI smooths it.
            let peak = mono.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            level_micro.store(linear_to_micro(peak), Ordering::Relaxed);
            let resampled = resampler.feed(&mono);
            for sample in resampled {
                accumulator.push(sample);
                if accumulator.len() < VOICE_FRAME_SAMPLES {
                    continue;
                }
                let frame: Vec<f32> = accumulator.drain(..VOICE_FRAME_SAMPLES).collect();
                match encoder.encode(&frame) {
                    Ok(encoded) => {
                        let _ = frames_tx.try_send(encoded);
                    }
                    Err(error) => {
                        bevy::log::warn!("voice encode dropped a frame: {error:#}");
                    }
                }
            }
        },
    );
    let stream = match build_result {
        Ok(stream) => stream,
        Err(error) => {
            let msg = format!("voice capture disabled: {error:#}");
            bevy::log::warn!("{msg}");
            let _ = ready_tx.send(Err(msg));
            return Ok(());
        }
    };
    if let Err(error) = stream.play() {
        let msg = format!("voice capture disabled: starting input stream: {error:#}");
        bevy::log::warn!("{msg}");
        let _ = ready_tx.send(Err(msg));
        return Ok(());
    }
    let _ = ready_tx.send(Ok(()));

    // Park until the controller asks us to stop (`Shutdown`) or the channel
    // closes (the main thread dropped its sender). Either way we tear down.
    let _ = cmd_rx.recv();
    drop(stream);
    Ok(())
}

/// Looks for a 48 kHz config first (preferred, no resampling needed), then
/// falls back to whatever the device picks as its default. We accept 1–2
/// channels and downmix in software; macOS in particular reports stereo for
/// most built-in mics even when the underlying signal is mono.
fn find_supported_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let configs = device
        .supported_input_configs()
        .context("listing input configs")?
        .collect::<Vec<_>>();

    let acceptable_format = |format: SampleFormat| {
        matches!(
            format,
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
        )
    };
    let acceptable_channels = |ch: u16| (1..=2).contains(&ch);

    // Preferred: a range that covers 48 kHz exactly.
    for range in &configs {
        if !acceptable_channels(range.channels()) || !acceptable_format(range.sample_format()) {
            continue;
        }
        if range.min_sample_rate().0 > VOICE_SAMPLE_RATE_HZ
            || range.max_sample_rate().0 < VOICE_SAMPLE_RATE_HZ
        {
            continue;
        }
        return Ok((*range).with_sample_rate(cpal::SampleRate(VOICE_SAMPLE_RATE_HZ)));
    }

    // Fallback: take the device's preferred default. We'll resample in software.
    match device.default_input_config() {
        Ok(config)
            if acceptable_channels(config.channels())
                && acceptable_format(config.sample_format()) =>
        {
            bevy::log::warn!(
                "voice capture: device default {} Hz; resampling to {} Hz",
                config.sample_rate().0,
                VOICE_SAMPLE_RATE_HZ
            );
            Ok(config)
        }
        Ok(config) => Err(anyhow!(
            "input device default config is unusable (channels={}, format={:?})",
            config.channels(),
            config.sample_format()
        )),
        Err(error) => Err(anyhow!("default input config unavailable: {error}")),
    }
}

fn build_stream<F>(
    device: &cpal::Device,
    sample_format: SampleFormat,
    config: &StreamConfig,
    mut on_samples: F,
) -> Result<cpal::Stream>
where
    F: FnMut(&[f32]) + Send + 'static,
{
    let err_fn = |error| {
        bevy::log::warn!("voice input stream error: {error}");
    };
    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
            config,
            move |data, _| on_samples(data),
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
            config,
            move |data, _| {
                let mut buf = Vec::with_capacity(data.len());
                for sample in data {
                    buf.push(*sample as f32 / i16::MAX as f32);
                }
                on_samples(&buf);
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream::<u16, _, _>(
            config,
            move |data, _| {
                let mut buf = Vec::with_capacity(data.len());
                for sample in data {
                    let centered = *sample as f32 - (u16::MAX as f32 / 2.0);
                    buf.push(centered / (u16::MAX as f32 / 2.0));
                }
                on_samples(&buf);
            },
            err_fn,
            None,
        ),
        other => return Err(anyhow!("unsupported sample format: {other:?}")),
    };
    stream.context("building voice input stream")
}

/// Average interleaved channel samples into a single mono buffer. Multiplies
/// by `gain` in the same pass so the encoder always sees the user's chosen
/// mic level.
fn downmix_to_mono(interleaved: &[f32], channels: usize, gain: f32) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.iter().map(|s| *s * gain).collect();
    }
    let frame_count = interleaved.len() / channels;
    let mut out = Vec::with_capacity(frame_count);
    let scale = gain / channels as f32;
    for frame_idx in 0..frame_count {
        let mut acc = 0.0f32;
        for ch in 0..channels {
            acc += interleaved[frame_idx * channels + ch];
        }
        out.push(acc * scale);
    }
    out
}

fn linear_to_micro(value: f32) -> u32 {
    (value.clamp(0.0, 4.0) * 1_000_000.0) as u32
}

fn micro_to_linear(value: u32) -> f32 {
    value as f32 / 1_000_000.0
}

/// Drains the capture channel without blocking. Returns each ready frame so
/// the caller can decide whether to ship it (PTT held) or drop it (released).
pub(crate) fn drain_frames(capture: &VoiceCapture) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while let Ok(frame) = capture.frames_rx.try_recv() {
        out.push(frame);
    }
    out
}
