//! Decoded-voice playback + spatial mix.
//!
//! A dedicated worker thread owns the cpal output stream (cpal's `Stream` is
//! `!Send` on macOS). The Bevy side decodes incoming Opus packets, pushes
//! mono PCM into a per-speaker ring buffer, and writes the desired spatial
//! gains. The audio callback reads everything through a `Mutex` and mixes
//! into the output buffer.
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
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::protocol::{ClientId, VOICE_SAMPLE_RATE_HZ};

use super::codec::VoiceDecoder;

/// Max samples to buffer per speaker. Beyond this we drop the oldest tail
/// rather than ballooning memory if a peer's voice arrives faster than we
/// can play it back (rare, but easy to defend against).
const MAX_BUFFERED_SAMPLES_PER_SPEAKER: usize = VOICE_SAMPLE_RATE_HZ as usize / 2; // 500 ms

/// Warmup target: don't start playing a speaker until we've buffered at
/// least this many samples (~100 ms = 5 Opus frames). The buffer drains
/// at audio-callback rate but refills at Bevy-frame rate, so the startup
/// cushion has to absorb (a) one full Bevy frame of jitter on the receive
/// side (~16 ms at 60 Hz, more when the frame skips), (b) network jitter,
/// and (c) the per-callback granularity. 100 ms is the sweet spot used by
/// most game voice systems, large enough to ride out routine schedule
/// hiccups, small enough to stay perceptually "immediate".
const PLAYBACK_WARMUP_SAMPLES: usize = (VOICE_SAMPLE_RATE_HZ as usize * 100) / 1000;

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

enum PlaybackCmd {
    Shutdown,
}

#[derive(Default)]
pub(crate) struct SpeakerSlot {
    pub(crate) decoder: Option<VoiceDecoder>,
    /// Decoded mono samples queued for playback.
    pub(crate) samples: VecDeque<f32>,
    pub(crate) gain_left: f32,
    pub(crate) gain_right: f32,
    /// Last sequence number we accepted from the wire. Used to drop
    /// reordered duplicates and to detect single-packet losses for FEC.
    pub(crate) last_sequence: Option<u16>,
    /// Jitter-buffer state. `false` until we've accumulated
    /// [`PLAYBACK_WARMUP_SAMPLES`] for the first time, the audio callback
    /// plays silence for this slot until then. Sticky once true; only
    /// resets if the buffer has been empty for
    /// [`PLAYBACK_RESET_AFTER_EMPTY_CALLBACKS`] callbacks in a row.
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
}

pub(crate) struct VoicePlayback {
    mixer: Arc<Mutex<Mixer>>,
    cmd_tx: Sender<PlaybackCmd>,
    worker: Option<JoinHandle<()>>,
}

impl VoicePlayback {
    pub(crate) fn spawn() -> Result<Self> {
        let mixer = Arc::new(Mutex::new(Mixer {
            speakers: HashMap::new(),
            output_gain: 1.0,
        }));
        let (cmd_tx, cmd_rx) = unbounded::<PlaybackCmd>();
        let (ready_tx, ready_rx) = unbounded::<()>();
        let mixer_clone = Arc::clone(&mixer);

        let worker = thread::Builder::new()
            .name("voice-playback".into())
            .spawn(move || {
                if let Err(error) = run_playback(mixer_clone, cmd_rx, ready_tx) {
                    bevy::log::warn!("voice playback stopped: {error:#}");
                }
            })
            .context("failed to spawn voice playback thread")?;

        ready_rx
            .recv()
            .context("voice playback thread failed to come up")?;

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
    /// gains. Decoding is done up front so the audio callback only touches
    /// the cheap PCM buffer.
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
                    push_samples(&mut slot.samples, &samples);
                }
                // Synthesise the remaining missing frames before this
                // one is decoded fresh below.
                for _ in 0..gap.saturating_sub(2) {
                    match decoder.decode_loss() {
                        Ok(samples) => push_samples(&mut slot.samples, &samples),
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
                Ok(samples) => push_samples(&mut slot.samples, &samples),
                Err(error) => {
                    bevy::log::trace!("voice decode failed: {error:#}");
                }
            }
        }
        slot.gain_left = gain_left;
        slot.gain_right = gain_right;
        slot.last_sequence = Some(sequence);
        if !slot.ready && slot.samples.len() >= PLAYBACK_WARMUP_SAMPLES {
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

fn push_samples(buffer: &mut VecDeque<f32>, samples: &[f32]) {
    buffer.extend(samples.iter().copied());
    while buffer.len() > MAX_BUFFERED_SAMPLES_PER_SPEAKER {
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
    mixer: Arc<Mutex<Mixer>>,
    cmd_rx: Receiver<PlaybackCmd>,
    ready_tx: Sender<()>,
) -> Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no default audio output device"))?;
    let supported = pick_output_config(&device)?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let channels = config.channels;
    let _ = ready_tx.send(());

    let stream = build_stream(&device, sample_format, &config, mixer, channels as usize)?;
    stream.play().context("starting voice output stream")?;

    // Park until the controller asks us to stop (or drops its sender).
    let _ = cmd_rx.recv();
    drop(stream);
    Ok(())
}

fn pick_output_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let configs = device
        .supported_output_configs()
        .context("listing output configs")?;
    let mut best: Option<cpal::SupportedStreamConfigRange> = None;
    for range in configs {
        if range.channels() < 1 || range.channels() > 2 {
            continue;
        }
        let format = range.sample_format();
        if !matches!(
            format,
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
        ) {
            continue;
        }
        let supports_48k = range.min_sample_rate().0 <= VOICE_SAMPLE_RATE_HZ
            && range.max_sample_rate().0 >= VOICE_SAMPLE_RATE_HZ;
        if !supports_48k {
            continue;
        }
        best = Some(match best {
            None => range,
            Some(current) if range.channels() > current.channels() => range,
            Some(current) => current,
        });
    }
    let chosen =
        best.ok_or_else(|| anyhow!("no output device supports 48 kHz mono/stereo playback"))?;
    Ok(chosen.with_sample_rate(cpal::SampleRate(VOICE_SAMPLE_RATE_HZ)))
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
        let mut buffer = VecDeque::new();
        let chunk = vec![0.0f32; MAX_BUFFERED_SAMPLES_PER_SPEAKER];
        push_samples(&mut buffer, &chunk);
        push_samples(&mut buffer, &chunk);
        assert_eq!(buffer.len(), MAX_BUFFERED_SAMPLES_PER_SPEAKER);
    }
}
