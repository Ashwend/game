//! Naive linear-interpolation resampler shared by capture and playback.
//!
//! Capture uses it to convert any input device rate up/down to the 48 kHz the
//! Opus codec expects; playback uses it to convert the decoder's 48 kHz output
//! down/up to whatever rate the output device actually runs at. Keeping a
//! single persistent instance per stream (its `cursor`/`last_sample` carry
//! across callback boundaries) is what stops clicks at buffer edges, so do not
//! construct a fresh resampler per audio callback.
//!
//! Voice quality is fine with linear interpolation because the source rate is
//! very often already 48 kHz (so `feed` is a copy), and the worst case if we
//! land on 44.1 kHz is "speech sounds slightly less crisp", not "speech is
//! unintelligible".

pub(crate) struct LinearResampler {
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
    pub(crate) fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            in_rate,
            out_rate,
            cursor: 0.0,
            last_sample: 0.0,
        }
    }

    pub(crate) fn feed(&mut self, input: &[f32]) -> Vec<f32> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_rate_is_a_passthrough() {
        let mut r = LinearResampler::new(48_000, 48_000);
        let input = [0.1, 0.2, 0.3, 0.4];
        assert_eq!(r.feed(&input), input.to_vec());
    }

    #[test]
    fn downsample_produces_roughly_proportional_count() {
        // 48k -> 44.1k over a long buffer lands within one sample of the ideal
        // ratio; the carried cursor absorbs the remainder on the next call.
        let mut r = LinearResampler::new(48_000, 44_100);
        let input = vec![0.5f32; 4_800];
        let out = r.feed(&input);
        let expected = (4_800.0 * 44_100.0 / 48_000.0) as usize;
        assert!((out.len() as isize - expected as isize).abs() <= 1);
        // A constant input stays constant through linear interpolation, except
        // the very first sample, which interpolates up from the resampler's
        // initial carried sample (0.0) - a one-sample warm-up, not a glitch.
        assert!(out.iter().skip(1).all(|s| (*s - 0.5).abs() < 1e-4));
    }

    #[test]
    fn upsample_produces_more_samples() {
        let mut r = LinearResampler::new(44_100, 48_000);
        let input = vec![0.25f32; 4_410];
        let out = r.feed(&input);
        assert!(out.len() > input.len());
        assert!(out.iter().all(|s| s.is_finite()));
    }
}
