//! Reusable fade-in / fade-out for any audio entity.
//!
//! The same fade machinery used by the menu music drives ambient-bed
//! cross-fades and looping emitter approach/retreat. One component, one
//! tick system — nobody has to re-implement linear interpolation against
//! Time and the `AudioSink::set_volume` API again.

use bevy::{
    audio::{AudioSink, AudioSinkPlayback, Volume},
    prelude::*,
};

/// Drive an attached [`AudioSink`] from its current volume toward a
/// target volume over `duration_secs`. Optionally despawns the entity
/// when the fade completes (use for fade-outs).
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct AudioFader {
    start: f32,
    target: f32,
    elapsed: f32,
    duration: f32,
    /// Despawn the entity when `elapsed >= duration`. Use for the "fade
    /// out and disappear" case (menu music transitioning, ambient
    /// emitter leaving range, looping sound retiring).
    despawn_on_complete: bool,
}

impl AudioFader {
    /// Fade from `start_volume` toward `target_volume` over `duration_secs`.
    pub(crate) fn new(start_volume: f32, target_volume: f32, duration_secs: f32) -> Self {
        Self {
            start: start_volume.max(0.0),
            target: target_volume.max(0.0),
            elapsed: 0.0,
            duration: duration_secs.max(f32::EPSILON),
            despawn_on_complete: false,
        }
    }

    /// Fade to zero and despawn when done — the typical "kill this loop"
    /// pattern.
    pub(crate) fn fade_out(current_volume: f32, duration_secs: f32) -> Self {
        Self {
            start: current_volume.max(0.0),
            target: 0.0,
            elapsed: 0.0,
            duration: duration_secs.max(f32::EPSILON),
            despawn_on_complete: true,
        }
    }

    fn progress(&self) -> f32 {
        (self.elapsed / self.duration).clamp(0.0, 1.0)
    }

    fn current_linear(&self) -> f32 {
        let t = self.progress();
        self.start + (self.target - self.start) * t
    }

    fn is_done(&self) -> bool {
        self.elapsed >= self.duration
    }
}

/// Advance every active fader. Drives the entity's [`AudioSink`] volume
/// each frame and, on completion, either drops the fader component (so
/// the entity stays at its target volume) or despawns the entity outright.
pub(crate) fn tick_audio_faders_system(
    mut commands: Commands,
    time: Res<Time>,
    mut faders: Query<(Entity, &mut AudioFader, Option<&mut AudioSink>)>,
) {
    let dt = time.delta_secs().max(0.0);
    for (entity, mut fader, sink) in &mut faders {
        fader.elapsed += dt;
        let level = fader.current_linear();
        if let Some(mut sink) = sink {
            sink.set_volume(Volume::Linear(level));
        }
        if fader.is_done() {
            if fader.despawn_on_complete {
                commands.entity(entity).despawn();
            } else {
                commands.entity(entity).remove::<AudioFader>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fader_progresses_linearly() {
        let mut fader = AudioFader::new(0.0, 1.0, 2.0);
        fader.elapsed = 1.0;
        assert!((fader.current_linear() - 0.5).abs() < 1e-5);
        fader.elapsed = 2.0;
        assert!((fader.current_linear() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn fade_out_targets_silence_and_marks_for_despawn() {
        let fader = AudioFader::fade_out(0.7, 1.0);
        assert_eq!(fader.target, 0.0);
        assert!(fader.despawn_on_complete);
    }

    #[test]
    fn progress_clamps_after_duration() {
        let mut fader = AudioFader::new(0.0, 1.0, 1.0);
        fader.elapsed = 10.0;
        assert!(fader.is_done());
        assert_eq!(fader.progress(), 1.0);
        assert_eq!(fader.current_linear(), 1.0);
    }
}
