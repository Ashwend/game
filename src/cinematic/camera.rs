//! Keyframed camera paths for cinematic shots.
//!
//! A path is a small set of `(time, eye, look)` keys; the client samples it
//! every frame while a shot plays and writes the result straight onto the
//! main camera transform. Position and look-at target interpolate on a
//! Catmull-Rom spline through the keys (ends clamped), which gives smooth,
//! drifting motion through the middle keys without hand-tuning tangents.
//! Dependency-light on purpose (only `Vec3` math), matching the shared
//! meteor-trajectory module, so tests can sample paths headlessly.

use bevy::math::Vec3;

/// One camera keyframe. `t` is seconds from shot start; keys must be listed
/// in strictly increasing `t` order (asserted by the script sanity test).
#[derive(Debug, Clone, Copy)]
pub struct CameraKey {
    pub t: f32,
    pub eye: Vec3,
    pub look: Vec3,
}

pub const fn key(t: f32, eye: Vec3, look: Vec3) -> CameraKey {
    CameraKey { t, eye, look }
}

/// A keyframed shot path. The final key's `t` is the shot duration.
#[derive(Debug, Clone, Copy)]
pub struct CameraPath {
    pub keys: &'static [CameraKey],
}

impl CameraPath {
    pub fn duration_seconds(&self) -> f32 {
        self.keys.last().map(|k| k.t).unwrap_or(0.0)
    }

    /// Sample the path at `t` seconds from shot start. Returns the eye
    /// position and the look-at target; `t` outside the keyed range clamps
    /// to the first / last key so a shot holds its final framing while the
    /// intermission runs.
    pub fn sample(&self, t: f32) -> (Vec3, Vec3) {
        let keys = self.keys;
        match keys {
            [] => (Vec3::new(0.0, 2.0, 0.0), Vec3::ZERO),
            [only] => (only.eye, only.look),
            _ => {
                let first = &keys[0];
                let last = &keys[keys.len() - 1];
                if t <= first.t {
                    return (first.eye, first.look);
                }
                if t >= last.t {
                    return (last.eye, last.look);
                }
                // Find the segment containing t.
                let mut segment = 0;
                for i in 0..keys.len() - 1 {
                    if t >= keys[i].t && t <= keys[i + 1].t {
                        segment = i;
                        break;
                    }
                }
                let k1 = &keys[segment];
                let k2 = &keys[segment + 1];
                let span = (k2.t - k1.t).max(f32::EPSILON);
                let s = ((t - k1.t) / span).clamp(0.0, 1.0);
                // Clamped neighbours for the spline ends.
                let k0 = &keys[segment.saturating_sub(1)];
                let k3 = &keys[(segment + 2).min(keys.len() - 1)];
                (
                    catmull_rom(k0.eye, k1.eye, k2.eye, k3.eye, s),
                    catmull_rom(k0.look, k1.look, k2.look, k3.look, s),
                )
            }
        }
    }
}

/// Standard uniform Catmull-Rom interpolation between `p1` and `p2`.
fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, s: f32) -> Vec3 {
    let s2 = s * s;
    let s3 = s2 * s;
    0.5 * ((2.0 * p1)
        + (p2 - p0) * s
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * s2
        + (3.0 * p1 - 3.0 * p2 + p3 - p0) * s3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_clamps_and_passes_through_keys() {
        const KEYS: &[CameraKey] = &[
            key(0.0, Vec3::new(0.0, 1.0, 0.0), Vec3::ZERO),
            key(5.0, Vec3::new(10.0, 2.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
            key(10.0, Vec3::new(20.0, 3.0, 5.0), Vec3::new(2.0, 0.0, 0.0)),
        ];
        let path = CameraPath { keys: KEYS };
        assert_eq!(path.duration_seconds(), 10.0);
        let (eye, _) = path.sample(-1.0);
        assert_eq!(eye, Vec3::new(0.0, 1.0, 0.0));
        let (eye, _) = path.sample(99.0);
        assert_eq!(eye, Vec3::new(20.0, 3.0, 5.0));
        // Catmull-Rom passes through the interior key exactly.
        let (eye, look) = path.sample(5.0);
        assert!((eye - Vec3::new(10.0, 2.0, 0.0)).length() < 1e-4);
        assert!((look - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn sample_is_continuous_across_segments() {
        const KEYS: &[CameraKey] = &[
            key(0.0, Vec3::ZERO, Vec3::ZERO),
            key(4.0, Vec3::new(8.0, 4.0, 0.0), Vec3::new(1.0, 1.0, 0.0)),
            key(8.0, Vec3::new(16.0, 0.0, 8.0), Vec3::new(2.0, 0.0, 2.0)),
        ];
        let path = CameraPath { keys: KEYS };
        let (a, _) = path.sample(3.999);
        let (b, _) = path.sample(4.001);
        assert!((a - b).length() < 0.05, "discontinuity at key: {a} vs {b}");
    }
}
