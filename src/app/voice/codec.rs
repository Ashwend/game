//! Thin wrappers around the libopus encoder/decoder. Centralizes the codec
//! configuration (sample rate, channels, bitrate, application profile) so
//! both halves of the pipeline stay in lockstep.

use anyhow::{Context, Result};
use opus::{Application, Bitrate, Channels, Decoder, Encoder};

use crate::protocol::{MAX_VOICE_FRAME_BYTES, VOICE_FRAME_SAMPLES, VOICE_SAMPLE_RATE_HZ};

/// Target bit-rate for the voice stream. 24 kbps in Opus's VoIP mode
/// produces wideband-quality speech (~6 kHz audio bandwidth) and stays
/// well below 4 KB/s per speaker even with packet overhead.
const VOICE_BITRATE_BPS: i32 = 24_000;

pub(crate) struct VoiceEncoder {
    encoder: Encoder,
    scratch: Vec<u8>,
}

impl VoiceEncoder {
    pub(crate) fn new() -> Result<Self> {
        let mut encoder = Encoder::new(VOICE_SAMPLE_RATE_HZ, Channels::Mono, Application::Voip)
            .context("failed to construct opus encoder")?;
        encoder
            .set_bitrate(Bitrate::Bits(VOICE_BITRATE_BPS))
            .context("failed to set opus bitrate")?;
        // In-band FEC + a small expected packet loss hint lets the decoder
        // reconstruct a single lost frame between two delivered ones, which
        // matters more than the ~5% bitrate hit on an unreliable channel.
        encoder
            .set_inband_fec(true)
            .context("failed to enable opus FEC")?;
        encoder
            .set_packet_loss_perc(5)
            .context("failed to set opus expected packet loss")?;
        Ok(Self {
            encoder,
            scratch: vec![0u8; MAX_VOICE_FRAME_BYTES],
        })
    }

    /// Encodes one 20 ms frame of mono f32 PCM into Opus bytes. Returns a
    /// fresh `Vec<u8>` so the caller can ship it through a channel without
    /// keeping our scratch buffer alive.
    pub(crate) fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>> {
        debug_assert_eq!(samples.len(), VOICE_FRAME_SAMPLES);
        let written = self
            .encoder
            .encode_float(samples, &mut self.scratch)
            .context("opus encode failed")?;
        Ok(self.scratch[..written].to_vec())
    }
}

pub(crate) struct VoiceDecoder {
    decoder: Decoder,
}

impl VoiceDecoder {
    pub(crate) fn new() -> Result<Self> {
        let decoder = Decoder::new(VOICE_SAMPLE_RATE_HZ, Channels::Mono)
            .context("failed to construct opus decoder")?;
        Ok(Self { decoder })
    }

    /// Decodes one Opus packet into a fresh `Vec<f32>` of mono 48 kHz PCM.
    /// `fec_recover = true` asks the decoder to synthesise a missing frame
    /// using the in-band FEC info carried by the next packet.
    pub(crate) fn decode(&mut self, packet: &[u8], fec_recover: bool) -> Result<Vec<f32>> {
        let mut out = vec![0.0f32; VOICE_FRAME_SAMPLES];
        let written = self
            .decoder
            .decode_float(packet, &mut out, fec_recover)
            .context("opus decode failed")?;
        out.truncate(written);
        Ok(out)
    }

    /// Generate one frame of packet-loss-concealment audio. Called when the
    /// receiver detects a sequence gap and wants to fill it with Opus's
    /// internal PLC synthesis (better-sounding than silence for short
    /// gaps). The Opus C API treats an empty packet + `decode_fec = false`
    /// as "synthesize one frame of concealment".
    pub(crate) fn decode_loss(&mut self) -> Result<Vec<f32>> {
        let mut out = vec![0.0f32; VOICE_FRAME_SAMPLES];
        let written = self
            .decoder
            .decode_float(&[], &mut out, false)
            .context("opus PLC failed")?;
        out.truncate(written);
        Ok(out)
    }
}
