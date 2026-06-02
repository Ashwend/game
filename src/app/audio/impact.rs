//! Tool-impact + miss + tree-fall audio.
//!
//! The local player's pending audio cues come out of [`GatherInputState`];
//! remote players' come through [`RemoteImpactEvent`]. Both flow into
//! [`PlaySound`] events on the central audio bus, choice of clip is a
//! `(ToolKind, SurfaceMaterial)` lookup against
//! [`super::manifest::impact_sound_for`].

use bevy::prelude::*;

use crate::app::state::{GatherInputState, RemoteImpactEvent};

use super::{
    library::PlaySound,
    manifest::{SoundId, impact_sound_for, impact_sound_for_player},
};

/// Drain queued impact + miss cues each frame and emit `PlaySound`
/// events. Spatial settings, picker state, polyphony cap, pitch jitter
/// are all the central play system's concern.
pub(crate) fn play_impact_sounds_system(
    mut gather_input: ResMut<GatherInputState>,
    mut remote_impacts: MessageReader<RemoteImpactEvent>,
    mut play: MessageWriter<PlaySound>,
) {
    if let Some(cue) = gather_input.take_pending_audio_cue() {
        let id = if cue.is_player_hit {
            impact_sound_for_player(cue.tool)
        } else {
            impact_sound_for(cue.tool, cue.surface)
        };
        if let Some(id) = id {
            play.write(PlaySound::at(id, cue.anchor));
        } else {
            // No dedicated impact clip for this (tool, surface). Fall
            // back to the miss whoosh so the swing isn't silent, the
            // ear notices a missing transient way faster than a quiet
            // one.
            play.write(PlaySound::non_spatial(SoundId::SwingMiss));
        }
    }
    if gather_input.take_pending_miss_audio() {
        play.write(PlaySound::non_spatial(SoundId::SwingMiss));
    }
    for event in remote_impacts.read() {
        let id = if event.is_player_hit {
            impact_sound_for_player(event.tool)
        } else {
            impact_sound_for(event.tool, event.surface)
        };
        if let Some(id) = id {
            play.write(PlaySound::at(id, event.anchor));
        }
    }
}

/// Emit the tree-fall crash sound at `anchor` (the trunk's base). The
/// audio entity is independent of the felling tree so playback survives
/// the trunk fading out and despawning when the animation finishes.
/// Convenience wrapper for the bare `PlaySound::at` call, callers can
/// still build the message manually if they need a gain offset.
#[allow(dead_code)]
pub(crate) fn emit_tree_fall_sound(play: &mut MessageWriter<PlaySound>, anchor: Vec3) {
    play.write(PlaySound::at(SoundId::TreeFall, anchor));
}

#[cfg(test)]
mod tests {
    use super::super::surface::SurfaceMaterial;
    use super::*;
    use crate::items::ToolKind;

    #[test]
    fn impact_table_is_independent_of_visual_effect_kind() {
        // The whole point of the audio rekey: we drive selection from
        // (tool, surface) regardless of what particle the visual system
        // chose to spawn.
        assert_eq!(
            impact_sound_for(ToolKind::Axe, SurfaceMaterial::Wood),
            Some(SoundId::ImpactAxeOnWood)
        );
        assert_eq!(
            impact_sound_for(ToolKind::Pickaxe, SurfaceMaterial::Coal),
            Some(SoundId::ImpactPickaxeOnCoal)
        );
        // Wrong-tool-on-deployable used to fall through to a miss
        // whoosh; the mixed-down pools cover those gaps now.
        assert_eq!(
            impact_sound_for(ToolKind::Axe, SurfaceMaterial::Stone),
            Some(SoundId::ImpactAxeGeneric)
        );
        assert_eq!(
            impact_sound_for(ToolKind::Pickaxe, SurfaceMaterial::Wood),
            Some(SoundId::ImpactPickaxeOnWood)
        );
    }
}
