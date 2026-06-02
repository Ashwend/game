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
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::protocol::{VOICE_FRAME_SAMPLES, VOICE_SAMPLE_RATE_HZ};

use super::codec::VoiceEncoder;

enum CaptureCmd {
    Shutdown,
}

/// Owns the cpal mic stream and ships encoded Opus frames out via
/// `frames_rx`. Spawned once when the voice subsystem starts and torn down
/// only when the app exits.
pub(crate) struct VoiceCapture {
    pub(crate) frames_rx: Receiver<Vec<u8>>,
    cmd_tx: Sender<CaptureCmd>,
    /// Mic gain as a fixed-point f32 (input_gain * 1_000_000). Atomic so the
    /// audio thread can read it on every callback without a lock.
    gain_micro: Arc<AtomicU32>,
    worker: Option<JoinHandle<()>>,
}

impl VoiceCapture {
    pub(crate) fn spawn() -> Result<Self> {
        let (frames_tx, frames_rx) = unbounded::<Vec<u8>>();
        let (cmd_tx, cmd_rx) = unbounded::<CaptureCmd>();
        let (ready_tx, ready_rx) = unbounded::<Result<(), String>>();
        let gain_micro = Arc::new(AtomicU32::new(linear_to_micro(1.0)));
        let gain_for_thread = Arc::clone(&gain_micro);

        let worker = thread::Builder::new()
            .name("voice-capture".into())
            .spawn(move || {
                if let Err(error) = run_capture(frames_tx, cmd_rx, ready_tx, gain_for_thread) {
                    bevy::log::warn!("voice capture stopped: {error:#}");
                }
            })
            .context("failed to spawn voice capture thread")?;

        // Wait for the worker to either open the stream or report a failure.
        // Without this `voice.available` would optimistically be true even
        // when no device matched and the thread had silently parked.
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                frames_rx,
                cmd_tx,
                gain_micro,
                worker: Some(worker),
            }),
            Ok(Err(error)) => Err(anyhow!("{error}")),
            Err(_) => Err(anyhow!(
                "voice capture thread exited before reporting status"
            )),
        }
    }

    pub(crate) fn set_input_gain(&self, gain: f32) {
        self.gain_micro
            .store(linear_to_micro(gain.clamp(0.0, 1.0)), Ordering::Relaxed);
    }
}

impl Drop for VoiceCapture {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(CaptureCmd::Shutdown);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn run_capture(
    frames_tx: Sender<Vec<u8>>,
    cmd_rx: Receiver<CaptureCmd>,
    ready_tx: Sender<Result<(), String>>,
    gain_micro: Arc<AtomicU32>,
) -> Result<()> {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(device) => device,
        None => {
            let _ = ready_tx.send(Err("no default audio input device".to_owned()));
            return Ok(());
        }
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

/// Naive linear-interpolation resampler. Voice quality is fine with this
/// because the source rate is almost always already 48 kHz (so it's a
/// no-op), and the failure mode if we ever land on 44.1 kHz is "speech
/// sounds slightly less crisp", not "speech is unintelligible".
struct LinearResampler {
    in_rate: u32,
    out_rate: u32,
    /// Sub-sample position into the current source frame; fractional part
    /// determines the next interpolation weight.
    cursor: f64,
    /// Most recent input sample carried across callback boundaries so the
    /// first output sample of a callback can interpolate against it.
    last_sample: f32,
}

impl LinearResampler {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            in_rate,
            out_rate,
            cursor: 0.0,
            last_sample: 0.0,
        }
    }

    fn feed(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        if self.in_rate == self.out_rate {
            self.last_sample = *input.last().unwrap_or(&0.0);
            return input.to_vec();
        }
        let step = self.in_rate as f64 / self.out_rate as f64;
        let mut out = Vec::with_capacity(((input.len() as f64) / step) as usize + 1);
        let mut position = self.cursor;
        while position < input.len() as f64 {
            let idx = position.floor() as usize;
            let frac = position - idx as f64;
            let prev = if idx == 0 {
                self.last_sample
            } else {
                input[idx - 1]
            };
            let next = input[idx];
            out.push((prev as f64 * (1.0 - frac) + next as f64 * frac) as f32);
            position += step;
        }
        self.cursor = position - input.len() as f64;
        self.last_sample = *input.last().unwrap_or(&self.last_sample);
        out
    }
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
