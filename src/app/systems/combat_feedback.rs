use bevy::prelude::*;

use crate::app::state::CombatFeedbackState;

/// Step the transient combat-feedback timers (hit marker + damage-direction
/// arrows) once per frame and drop anything that has expired. Mirrors how
/// `CameraImpactKick` is advanced from the camera system: pure timer decay, no
/// gameplay authority. Runs unconditionally so a marker keeps fading even if a
/// menu opens the same frame it was triggered.
pub(crate) fn tick_combat_feedback_system(
    time: Res<Time>,
    mut feedback: ResMut<CombatFeedbackState>,
) {
    feedback.advance(time.delta_secs());
}
