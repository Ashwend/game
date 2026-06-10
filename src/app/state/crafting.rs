//! UI-local state for the crafting screen. Everything authoritative lives
//! server-side and arrives in the per-tick snapshot, this resource only
//! tracks transient widget state (search text, category filter, scroll).

use std::collections::HashMap;

use bevy::prelude::Resource;

use crate::{crafting::RecipeCategory, protocol::CraftingJobId};

#[derive(Resource, Debug, Clone, Default)]
pub(crate) struct CraftingUiState {
    /// Plain-text filter applied case-insensitively to recipe name and
    /// description.
    pub(crate) search: String,
    /// Optional category filter. `None` means "all categories".
    pub(crate) category_filter: Option<RecipeCategory>,
    /// When true, hide recipes the player cannot currently craft. Quietly
    /// useful once the registry grows past a screenful.
    pub(crate) only_craftable: bool,
    /// Reset on each open of the crafting screen. Used by the renderer to
    /// scroll the recipe list back to the top, without this, a player
    /// who scrolled mid-list, closed, then reopened would re-enter at
    /// their last scroll position which feels disorienting.
    pub(crate) scroll_reset_pending: bool,
    /// Per-recipe batch-quantity buffer. Stored as a `String` (not a
    /// `u16`) so the player can type freely, including transiently
    /// invalid values like an empty string or a number that exceeds
    /// what's currently craftable. The recipe row parses the buffer on
    /// each frame, clamps it where the +/- buttons act, and disables the
    /// Craft button when the typed value can't be honoured. Keyed by the
    /// recipe's `&'static str` id so the row doesn't allocate when it
    /// only needs to *read* the current quantity.
    pub(crate) quantities: HashMap<&'static str, String>,
}

impl CraftingUiState {
    /// Reset the transient browser view to a fresh-open state: clear the
    /// search filter and scroll the recipe list back to the top. Shared by
    /// the `C` hotkey, the "open workbench" path, and the tab-bar switch into
    /// the crafting tab so every entry behaves the same.
    pub(crate) fn reset_browser(&mut self) {
        self.search.clear();
        self.scroll_reset_pending = true;
    }
}

/// Client-only smoothing state for the queue HUD progress bars.
///
/// The server only ships `progress_ticks` at the snapshot cadence (20 Hz).
/// Rendering that raw value at 60+ fps gives a visible 50 ms staircase on
/// every bar, the player perceives stutter even though the underlying
/// math is correct. We anchor a baseline each time we see a new
/// `progress_ticks` value, then advance the rendered fraction off the
/// local clock between snapshots. The next snapshot rebases the anchor,
/// so accumulated drift is bounded by one server frame.
#[derive(Resource, Debug, Default)]
pub(crate) struct CraftingHudState {
    pub(crate) progress: HashMap<CraftingJobId, ProgressBaseline>,
    /// Job ids the player explicitly cancelled from the queue HUD. The
    /// craft-complete cue consults this so a cancelled job vanishing from
    /// the replicated queue doesn't chime like a completion. Bounded by
    /// [`Self::note_cancel_requested`]; ids are never reused within a
    /// session (the server allocator only wraps after `u64::MAX` jobs),
    /// so stale entries are harmless.
    pub(crate) recently_cancelled: Vec<CraftingJobId>,
}

impl CraftingHudState {
    /// Remember that the player asked to cancel `job_id`, so its
    /// disappearance from the queue stays silent. Kept tiny: the list
    /// only needs to cover cancels still in flight to the server.
    pub(crate) fn note_cancel_requested(&mut self, job_id: CraftingJobId) {
        self.recently_cancelled.push(job_id);
        let overflow = self.recently_cancelled.len().saturating_sub(16);
        if overflow > 0 {
            self.recently_cancelled.drain(..overflow);
        }
    }

    /// True (and forgets the entry) when `job_id` was locally cancelled.
    pub(crate) fn consume_cancelled(&mut self, job_id: CraftingJobId) -> bool {
        if let Some(index) = self
            .recently_cancelled
            .iter()
            .position(|cancelled| *cancelled == job_id)
        {
            self.recently_cancelled.swap_remove(index);
            return true;
        }
        false
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProgressBaseline {
    /// Most recent `progress_ticks` we observed from the server. Stored so
    /// we can detect when the server has advanced the job and rebase the
    /// interpolation anchor.
    pub(crate) observed_ticks: u32,
    /// Cached `total_ticks` for the job. Stays constant for a job's life,
    /// but worth storing so the HUD can interpolate without re-deriving
    /// it from the recipe registry every frame.
    pub(crate) total_ticks: u32,
    /// egui clock value (`ctx.input(|i| i.time)`) when `observed_ticks`
    /// was last seen. The interpolated fraction is computed as
    /// `(observed_ticks + (now - observed_at) * tick_rate) / total_ticks`.
    pub(crate) observed_at_secs: f64,
}
