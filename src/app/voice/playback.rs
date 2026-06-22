//! Decoded-voice playback + spatial mix.
//!
//! A dedicated worker thread owns the cpal output stream (cpal's `Stream` is
//! `!Send` on macOS). The Bevy side decodes incoming Opus packets, resamples
//! the 48 kHz decoder output to whatever rate the output device runs at,
//! pushes the result into a per-speaker ring buffer, and writes the desired
//! spatial gains. The audio callback reads everything through a `Mutex` and
//! mixes into the output buffer.
//!
//! Resampling happens here on the Bevy thread (in [`VoicePlayback::submit_packet`]),
//! not in the audio callback, so the callback stays allocation-free and the
//! per-speaker resampler keeps a single persistent cursor (no clicks at frame
//! edges). The common case is a 48 kHz output device, where the resampler is a
//! no-op passthrough.
//!
//! Tolerating a non-48 kHz output device is the load-bearing fix for the
//! one-way-audio bug: a listener whose default output advertised no exact
//! 48 kHz config (a 44.1 kHz DAC, a >2-channel HDMI/AirPlay/aggregate device)
//! used to fail playback init outright and silently hear nobody while their
//! own mic still worked.
//!
//! Mixing is intentionally simple, per-speaker stereo gains, no HRTF, no
//! Doppler. It's the right baseline for prototype quality and bumps the CPU
//! footprint into the "negligible per frame" range.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow};
use cpal::{
    SampleFormat, StreamConfig,
    traits::{DeviceTrait, StreamTrait},
};
use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::protocol::{ClientId, VOICE_SAMPLE_RATE_HZ};

use super::codec::VoiceDecoder;
use super::devices::resolve_output_device;
use super::resample::LinearResampler;

/// Maximum frames we'll synthesise via Opus packet-loss concealment when
/// a sequence gap is observed. Beyond this we assume the talker actually
/// paused (or the network died) and treat the gap as a stop rather than a
/// loss. Five frames = 100 ms of PLC output, which Opus handles cleanly;
/// past that the synthesised audio starts to sound like a ringing tone.
const MAX_PLC_FRAMES: u16 = 5;

/// Once a speaker is `ready`, keep playing through transient underruns
/// (the audio callback briefly finds the buffer empty between packets)
/// and only re-arm the warmup gate when *no* packets have arrived for an
/// extended window. Tracked as a count of consecutive empty audio
/// callbacks; conservative because most flicker in earlier iterations
/// came from re-arming on every brief dip.
const PLAYBACK_RESET_AFTER_EMPTY_CALLBACKS: u32 = 60;

/// Per-speaker buffer cap (~500 ms at the output rate). Beyond this we drop the
/// oldest tail rather than ballooning memory if a peer's voice arrives faster
/// than we can play it back.
fn max_buffered_samples(output_rate: u32) -> usize {
    output_rate as usize / 2
}

/// Warmup target (~100 ms = 5 Opus frames at the output rate): don't start
/// playing a speaker until we've buffered at least this much. The buffer drains
/// at audio-callback rate but refills at Bevy-frame rate, so the startup
/// cushion has to absorb (a) one full Bevy frame of jitter on the receive side
/// (~16 ms at 60 Hz, more when the frame skips), (b) network jitter, and (c)
/// the per-callback granularity. 100 ms is the sweet spot used by most game
/// voice systems, large enough to ride out routine schedule hiccups, small
/// enough to stay perceptually "immediate".
fn warmup_samples(output_rate: u32) -> usize {
    (output_rate as usize * 100) / 1000
}

enum PlaybackCmd {
    Shutdown,
}

#[derive(Default)]
pub(crate) struct SpeakerSlot {
    pub(crate) decoder: Option<VoiceDecoder>,
    /// Resampler converting the decoder's 48 kHz output to the output device
    /// rate. `None` while the output runs at 48 kHz (passthrough) or before
    /// the first frame; created lazily on first non-48 kHz push so its cursor
    /// persists across frames and never clicks at a frame boundary.
    pub(crate) out_resampler: Option<LinearResampler>,
    /// Output-rate mono samples queued for playback.
    pub(crate) samples: VecDeque<f32>,
    pub(crate) gain_left: f32,
    pub(crate) gain_right: f32,
    /// Last sequence number we accepted from the wire. Used to drop
    /// reordered duplicates and to detect single-packet losses for FEC.
    pub(crate) last_sequence: Option<u16>,
    /// Jitter-buffer state. `false` until we've accumulated the warmup target
    /// for the first time, the audio callback plays silence for this slot
    /// until then. Sticky once true; only resets if the buffer has been empty
    /// for [`PLAYBACK_RESET_AFTER_EMPTY_CALLBACKS`] callbacks in a row.
    pub(crate) ready: bool,
    /// Running count of consecutive audio callbacks that found the buffer
    /// empty. Zeroes the moment any sample is consumed.
    pub(crate) empty_callback_streak: u32,
}

#[derive(Default)]
pub(crate) struct Mixer {
    pub(crate) speakers: HashMap<ClientId, SpeakerSlot>,
    /// Master output gain, settable via [`VoicePlayback::set_output_gain`].
    pub(crate) output_gain: f32,
    /// The output device's sample rate, written once by the worker thread when
    /// the stream opens. `submit_packet` resamples 48 kHz to this; the audio
    /// callback consumes samples that are already at this rate.
    pub(crate) output_rate: u32,
}

pub(crate) struct VoicePlayback {
    mixer: Arc<Mutex<Mixer>>,
    cmd_tx: Sender<PlaybackCmd>,
    worker: Option<JoinHandle<()>>,
}

impl VoicePlayback {
    /// Opens the output stream. `device_name` selects a specific output device
    /// by its cpal name; `None` (or a name that no longer matches) uses the
    /// system default. Returns `Err` if the device or stream could not be
    /// opened, so the caller can surface "you won't hear other players" rather
    /// than silently swallowing the failure.
    pub(crate) fn spawn(device_name: Option<String>) -> Result<Self> {
        let mixer = Arc::new(Mutex::new(Mixer {
            speakers: HashMap::new(),
            output_gain: 1.0,
            output_rate: VOICE_SAMPLE_RATE_HZ,
        }));
        let (cmd_tx, cmd_rx) = unbounded::<PlaybackCmd>();
        let (ready_tx, ready_rx) = unbounded::<Result<(), String>>();
        let mixer_clone = Arc::clone(&mixer);

        let worker = thread::Builder::new()
            .name("voice-playback".into())
            .spawn(move || {
                if let Err(error) = run_playback(device_name, mixer_clone, cmd_rx, ready_tx) {
                    bevy::log::warn!("voice playback stopped: {error:#}");
                }
            })
            .context("failed to spawn voice playback thread")?;

        // Unlike capture, blocking here is fine: opening an output-only stream
        // does not raise an OS permission prompt. Crucially we now wait for the
        // *real* outcome (stream built AND playing), so a failed output device
        // surfaces as `Err` instead of a silent dead worker.
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => return Err(anyhow!(msg)),
            Err(_) => return Err(anyhow!("voice playback thread exited before reporting status")),
        }

        Ok(Self {
            mixer,
            cmd_tx,
            worker: Some(worker),
        })
    }

    pub(crate) fn set_output_gain(&self, gain: f32) {
        if let Ok(mut guard) = self.mixer.lock() {
            guard.output_gain = gain.clamp(0.0, 4.0);
        }
    }

    /// Pushes a decoded frame onto a speaker's queue with updated spatial
    /// gains. Decoding and resampling are done up front so the audio callback
    /// only touches the cheap PCM buffer.
    pub(crate) fn submit_packet(
        &self,
        speaker: ClientId,
        sequence: u16,
        packet: &[u8],
        gain_left: f32,
        gain_right: f32,
    ) {
        let Ok(mut guard) = self.mixer.lock() else {
            return;
        };
        let output_rate = guard.output_rate;
        let max_buffered = max_buffered_samples(output_rate);
        let warmup = warmup_samples(output_rate);
        let slot = guard.speakers.entry(speaker).or_default();
        if slot.decoder.is_none() {
            slot.decoder = match VoiceDecoder::new() {
                Ok(decoder) => Some(decoder),
                Err(error) => {
                    bevy::log::warn!("voice decoder construct failed: {error:#}");
                    return;
                }
            };
        }

        // Drop strict duplicates and out-of-order arrivals older than the
        // last accepted packet. `UnorderedUnreliable` means we can get
        // reorder spikes; without this guard a delayed packet would feed
        // stale audio in *after* its successor, which sounds awful.
        if let Some(prev) = slot.last_sequence
            && short_seq_le(sequence, prev)
        {
            return;
        }

        // Bridge any sequence gap with Opus's packet-loss concealment so
        // the listener hears continuous speech instead of a hole. The first
        // missing frame uses the *next* packet's in-band FEC payload (much
        // higher quality than pure PLC); any further missing frames fall
        // back to synthesis. Past `MAX_PLC_FRAMES` we assume the talker
        // actually stopped and skip ahead.
        if let Some(prev) = slot.last_sequence {
            let gap = sequence.wrapping_sub(prev);
            if gap > 1
                && gap <= MAX_PLC_FRAMES
                && let Some(decoder) = slot.decoder.as_mut()
            {
                // FEC reconstructs frame (prev + 1) from this packet's
                // payload, the cheapest one-frame fill we can do.
                if let Ok(samples) = decoder.decode(packet, true) {
                    push_decoded(
                        &mut slot.out_resampler,
                        &mut slot.samples,
                        &samples,
                        output_rate,
                        max_buffered,
                    );
                }
                // Synthesise the remaining missing frames before this
                // one is decoded fresh below.
                for _ in 0..gap.saturating_sub(2) {
                    match decoder.decode_loss() {
                        Ok(samples) => push_decoded(
                            &mut slot.out_resampler,
                            &mut slot.samples,
                            &samples,
                            output_rate,
                            max_buffered,
                        ),
                        Err(error) => {
                            bevy::log::trace!("voice PLC fill failed: {error:#}");
                            break;
                        }
                    }
                }
            }
        }

        if let Some(decoder) = slot.decoder.as_mut() {
            match decoder.decode(packet, false) {
                Ok(samples) => push_decoded(
                    &mut slot.out_resampler,
                    &mut slot.samples,
                    &samples,
                    output_rate,
                    max_buffered,
                ),
                Err(error) => {
                    bevy::log::trace!("voice decode failed: {error:#}");
                }
            }
        }
        slot.gain_left = gain_left;
        slot.gain_right = gain_right;
        slot.last_sequence = Some(sequence);
        if !slot.ready && slot.samples.len() >= warmup {
            slot.ready = true;
        }
    }

    /// Wipes any state for a speaker, e.g. when they disconnect or move
    /// out of audible range. Keeps the speaker list bounded.
    pub(crate) fn forget_speaker(&self, speaker: ClientId) {
        if let Ok(mut guard) = self.mixer.lock() {
            guard.speakers.remove(&speaker);
        }
    }

    /// Drops every active speaker slot at once. Used when the player turns
    /// voice chat off mid-session so any half-buffered speech goes quiet
    /// immediately instead of trailing off as the ring buffers drain.
    pub(crate) fn forget_all(&self) {
        if let Ok(mut guard) = self.mixer.lock() {
            guard.speakers.clear();
        }
    }
}

impl Drop for VoicePlayback {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(PlaybackCmd::Shutdown);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

/// Resample a decoded 48 kHz frame to the output rate (a no-op passthrough at
/// 48 kHz) and append it to the speaker buffer, trimming the oldest tail past
/// the cap. Takes the slot's fields disjointly so it can be called while the
/// slot's decoder is borrowed.
fn push_decoded(
    out_resampler: &mut Option<LinearResampler>,
    buffer: &mut VecDeque<f32>,
    decoded: &[f32],
    output_rate: u32,
    max_buffered: usize,
) {
    if output_rate == VOICE_SAMPLE_RATE_HZ {
        push_samples(buffer, decoded, max_buffered);
        return;
    }
    let resampler = out_resampler
        .get_or_insert_with(|| LinearResampler::new(VOICE_SAMPLE_RATE_HZ, output_rate));
    let resampled = resampler.feed(decoded);
    push_samples(buffer, &resampled, max_buffered);
}

fn push_samples(buffer: &mut VecDeque<f32>, samples: &[f32], max_buffered: usize) {
    buffer.extend(samples.iter().copied());
    while buffer.len() > max_buffered {
        // Discard the oldest tail, preferring a tiny truncation to ever-
        // increasing latency. In practice this only fires if the OS audio
        // graph stalls for hundreds of ms.
        buffer.pop_front();
    }
}

/// True when `a` is "less than or equal to" `b` in the 16-bit cyclic sense,
/// using half the namespace as the comparison window. Lets the mixer reject
/// strictly reordered duplicates without breaking when the sequence wraps.
fn short_seq_le(a: u16, b: u16) -> bool {
    let diff = b.wrapping_sub(a);
    diff != 0 && diff < (u16::MAX / 2)
}

fn run_playback(
    device_name: Option<String>,
    mixer: Arc<Mutex<Mixer>>,
    cmd_rx: Receiver<PlaybackCmd>,
    ready_tx: Sender<Result<(), String>>,
) -> Result<()> {
    let host = cpal::default_host();
    let Some(device) = resolve_output_device(&host, device_name.as_deref()) else {
        let _ = ready_tx.send(Err("no audio output device".to_owned()));
        return Ok(());
    };
    let supported = match pick_output_config(&device) {
        Ok(config) => config,
        Err(error) => {
            let msg = format!("voice playback disabled: {error:#}");
            bevy::log::warn!("{msg}");
            let _ = ready_tx.send(Err(msg));
            return Ok(());
        }
    };
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let channels = config.channels;
    let output_rate = config.sample_rate.0;
    bevy::log::info!(
        "voice playback: device={:?} format={:?} rate={} channels={}",
        device.name().ok(),
        sample_format,
        output_rate,
        channels
    );
    if let Ok(mut guard) = mixer.lock() {
        guard.output_rate = output_rate;
    }

    let stream = match build_stream(&device, sample_format, &config, mixer, channels as usize) {
        Ok(stream) => stream,
        Err(error) => {
            let msg = format!("voice playback disabled: {error:#}");
            bevy::log::warn!("{msg}");
            let _ = ready_tx.send(Err(msg));
            return Ok(());
        }
    };
    if let Err(error) = stream.play() {
        let msg = format!("voice playback disabled: starting output stream: {error:#}");
        bevy::log::warn!("{msg}");
        let _ = ready_tx.send(Err(msg));
        return Ok(());
    }
    // Only now, with the stream actually built AND playing, do we report
    // success. Reporting earlier (the old bug) let a build/play failure pass as
    // a live playback, so a dead output silently dropped every decoded frame.
    let _ = ready_tx.send(Ok(()));

    // Park until the controller asks us to stop (or drops its sender).
    let _ = cmd_rx.recv();
    drop(stream);
    Ok(())
}

/// Pick an output config. Prefers a config that natively covers 48 kHz (so the
/// mixer skips resampling), preferring stereo for spatial panning. Falls back
/// to the device's default config at its native rate, which the mixer then
/// resamples to. The fallback is what keeps a 44.1 kHz-only / >2-channel
/// default output from silently breaking playback.
fn pick_output_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let acceptable_format = |format: SampleFormat| {
        matches!(
            format,
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
        )
    };
    let acceptable_channels = |ch: u16| (1..=2).contains(&ch);

    let configs = device
        .supported_output_configs()
        .context("listing output configs")?
        .collect::<Vec<_>>();

    // Preferred: a range covering 48 kHz, preferring more channels (stereo).
    let mut best: Option<&cpal::SupportedStreamConfigRange> = None;
    for range in &configs {
        if !acceptable_channels(range.channels()) || !acceptable_format(range.sample_format()) {
            continue;
        }
        if range.min_sample_rate().0 > VOICE_SAMPLE_RATE_HZ
            || range.max_sample_rate().0 < VOICE_SAMPLE_RATE_HZ
        {
            continue;
        }
        best = Some(match best {
            None => range,
            Some(current) if range.channels() > current.channels() => range,
            Some(current) => current,
        });
    }
    if let Some(range) = best {
        return Ok((*range).with_sample_rate(cpal::SampleRate(VOICE_SAMPLE_RATE_HZ)));
    }

    // Fallback: the device's preferred default config at its native rate. The
    // mixer resamples 48 kHz -> this rate per speaker.
    match device.default_output_config() {
        Ok(config)
            if acceptable_channels(config.channels())
                && acceptable_format(config.sample_format()) =>
        {
            bevy::log::warn!(
                "voice playback: device default {} Hz; resampling {} -> {} Hz",
                config.sample_rate().0,
                VOICE_SAMPLE_RATE_HZ,
                config.sample_rate().0
            );
            Ok(config)
        }
        Ok(config) => Err(anyhow!(
            "output device default config is unusable (channels={}, format={:?})",
            config.channels(),
            config.sample_format()
        )),
        Err(error) => Err(anyhow!("default output config unavailable: {error}")),
    }
}

fn build_stream(
    device: &cpal::Device,
    sample_format: SampleFormat,
    config: &StreamConfig,
    mixer: Arc<Mutex<Mixer>>,
    channels: usize,
) -> Result<cpal::Stream> {
    let err_fn = |error| bevy::log::warn!("voice output stream error: {error}");
    let stream = match sample_format {
        SampleFormat::F32 => {
            let mixer = Arc::clone(&mixer);
            device.build_output_stream::<f32, _, _>(
                config,
                move |out, _| {
                    fill_output_f32(out, channels, &mixer);
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let mixer = Arc::clone(&mixer);
            device.build_output_stream::<i16, _, _>(
                config,
                move |out, _| {
                    let mut tmp = vec![0.0f32; out.len()];
                    fill_output_f32(&mut tmp, channels, &mixer);
                    for (dst, src) in out.iter_mut().zip(tmp.iter()) {
                        let clamped = src.clamp(-1.0, 1.0);
                        *dst = (clamped * i16::MAX as f32) as i16;
                    }
                },
                err_fn,
                None,
            )
        }
        SampleFormat::U16 => {
            let mixer = Arc::clone(&mixer);
            device.build_output_stream::<u16, _, _>(
                config,
                move |out, _| {
                    let mut tmp = vec![0.0f32; out.len()];
                    fill_output_f32(&mut tmp, channels, &mixer);
                    for (dst, src) in out.iter_mut().zip(tmp.iter()) {
                        let clamped = (src.clamp(-1.0, 1.0) + 1.0) * 0.5;
                        *dst = (clamped * u16::MAX as f32) as u16;
                    }
                },
                err_fn,
                None,
            )
        }
        other => return Err(anyhow!("unsupported output sample format: {other:?}")),
    };
    stream.context("building voice output stream")
}

fn fill_output_f32(out: &mut [f32], channels: usize, mixer: &Mutex<Mixer>) {
    // Zero first so unused frames stay silent when no speakers are active.
    for s in out.iter_mut() {
        *s = 0.0;
    }

    let Ok(mut guard) = mixer.lock() else {
        return;
    };
    let output_gain = guard.output_gain;
    if channels == 0 {
        return;
    }
    let frames = out.len() / channels;
    for slot in guard.speakers.values_mut() {
        if !slot.ready {
            // Still warming up after a fresh start or a sustained gap. Play
            // silence for this slot; `submit_packet` flips `ready` once
            // enough samples are buffered.
            continue;
        }
        let g_left = slot.gain_left * output_gain;
        let g_right = slot.gain_right * output_gain;
        let stereo = channels >= 2;
        let mut consumed_any = false;
        for frame_idx in 0..frames {
            let Some(sample) = slot.samples.pop_front() else {
                break;
            };
            consumed_any = true;
            let base = frame_idx * channels;
            if stereo {
                out[base] += sample * g_left;
                out[base + 1] += sample * g_right;
            } else {
                let mono = sample * ((g_left + g_right) * 0.5);
                out[base] += mono;
            }
        }
        if consumed_any {
            slot.empty_callback_streak = 0;
        } else {
            slot.empty_callback_streak = slot.empty_callback_streak.saturating_add(1);
            // Only re-arm warmup after a sustained empty window, a single
            // empty callback between packets is normal jitter, not a stop.
            if slot.empty_callback_streak >= PLAYBACK_RESET_AFTER_EMPTY_CALLBACKS {
                slot.ready = false;
            }
        }
    }

    // Final guard: hard-clamp the mixed output so multiple loud speakers
    // (or a panning quirk near unity) can't produce a clipped sample, which
    // is one of the audible-distortion sources the player was reporting.
    for sample in out.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_seq_le_handles_wrap() {
        assert!(short_seq_le(5, 6));
        assert!(short_seq_le(u16::MAX, 0)); // wrap forward
        assert!(!short_seq_le(6, 5));
        assert!(!short_seq_le(0, u16::MAX)); // wrap backward
    }

    #[test]
    fn push_samples_trims_to_cap() {
        let cap = max_buffered_samples(VOICE_SAMPLE_RATE_HZ);
        let mut buffer = VecDeque::new();
        let chunk = vec![0.0f32; cap];
        push_samples(&mut buffer, &chunk, cap);
        push_samples(&mut buffer, &chunk, cap);
        assert_eq!(buffer.len(), cap);
    }

    #[test]
    fn push_decoded_passthrough_at_48k() {
        let mut resampler = None;
        let mut buffer = VecDeque::new();
        let frame = vec![0.5f32; 960];
        push_decoded(
            &mut resampler,
            &mut buffer,
            &frame,
            VOICE_SAMPLE_RATE_HZ,
            max_buffered_samples(VOICE_SAMPLE_RATE_HZ),
        );
        // No resampler is created on the 48 kHz passthrough path.
        assert!(resampler.is_none());
        assert_eq!(buffer.len(), 960);
    }

    #[test]
    fn push_decoded_resamples_to_44100() {
        let out_rate = 44_100;
        let mut resampler = None;
        let mut buffer = VecDeque::new();
        let frame = vec![0.25f32; 960];
        push_decoded(
            &mut resampler,
            &mut buffer,
            &frame,
            out_rate,
            max_buffered_samples(out_rate),
        );
        // A resampler is created and produces fewer samples (48k -> 44.1k).
        assert!(resampler.is_some());
        assert!(buffer.len() < 960 && !buffer.is_empty());
        assert!(buffer.iter().all(|s| s.is_finite()));
    }
}
