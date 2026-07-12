//! Craft-completion audio cue.
//!
//! The server doesn't announce completions on a dedicated wire message;
//! a finished job simply leaves the replicated `PlayerCraftingState`
//! (its output lands in the inventory and a "Crafted X" toast follows).
//! This watcher diffs the replicated job list against the previous
//! frame's ids: a job that vanished without a locally-recorded cancel
//! (see `CraftingHudState::note_cancel_requested`) completed, so the
//! craft-complete cue fires. Cancels stay silent.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    analytics::{Analytics, Event},
    app::{
        audio::{PlaySound, SoundId},
        state::{CraftingHudState, LocalPlayerState},
    },
    crafting::RecipeId,
    protocol::CraftingJobId,
};

/// Jobs seen in the replicated crafting queue last frame, mapped to their
/// recipe id so a vanished (completed) job can be attributed to its recipe for
/// the `craft_completed` analytics event.
#[derive(Resource, Default)]
pub(crate) struct CraftCompletionWatch {
    previous_jobs: HashMap<CraftingJobId, RecipeId>,
}

pub(crate) fn craft_complete_cue_system(
    local_player: Res<LocalPlayerState>,
    mut watch: ResMut<CraftCompletionWatch>,
    mut hud_state: ResMut<CraftingHudState>,
    mut play_sound: MessageWriter<PlaySound>,
    analytics: Res<Analytics>,
) {
    let Some(private) = local_player.private.as_ref() else {
        // Disconnected: forget everything so jobs restored on the next
        // session don't read as "vanished" completions.
        if !watch.previous_jobs.is_empty() {
            watch.previous_jobs.clear();
        }
        return;
    };

    let current: HashMap<CraftingJobId, RecipeId> = private
        .crafting
        .jobs
        .iter()
        .map(|job| (job.job_id, job.recipe_id.clone()))
        .collect();

    // One cue per frame even if a batch of jobs completed in the same
    // replication tick; overlapping identical chimes just sound louder. The
    // analytics event, in contrast, fires once per completed recipe (a batch of
    // five swords is five `craft_completed` events, matching per-completion
    // dashboards).
    let mut completed_any = false;
    for (job_id, recipe_id) in watch.previous_jobs.iter() {
        if !current.contains_key(job_id) && !hud_state.consume_cancelled(*job_id) {
            completed_any = true;
            analytics.track(Event::CraftCompleted {
                recipe_id: recipe_id.as_ref().to_owned(),
            });
        }
    }
    if completed_any {
        play_sound.write(PlaySound::non_spatial(SoundId::CraftComplete));
    }

    if watch.previous_jobs != current {
        watch.previous_jobs = current;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_note_is_consumed_once() {
        let mut hud = CraftingHudState::default();
        hud.note_cancel_requested(7);
        assert!(hud.consume_cancelled(7), "first lookup consumes the note");
        assert!(!hud.consume_cancelled(7), "second lookup finds nothing");
    }

    #[test]
    fn cancel_notes_are_bounded() {
        let mut hud = CraftingHudState::default();
        for job_id in 0..100u64 {
            hud.note_cancel_requested(job_id);
        }
        assert!(hud.recently_cancelled.len() <= 16);
        // The most recent cancels are the ones kept.
        assert!(hud.consume_cancelled(99));
        assert!(!hud.consume_cancelled(0));
    }
}
