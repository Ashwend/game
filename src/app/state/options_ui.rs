//! Cross-frame UI state for the options panel — which tab is active, and if
//! the user has a rebind capture in flight. Kept separate from `MenuState`
//! because it's a UI-only artifact: leaving the panel and reopening it
//! should restore the previous tab, but the rebind capture must reset.

use bevy::prelude::*;

use super::{KeyAction, KeyBindingSlot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum OptionsTab {
    #[default]
    General,
    Display,
    Graphics,
    Audio,
    Voice,
    Controls,
    Keybindings,
}

impl OptionsTab {
    pub(crate) const ALL: [Self; 7] = [
        Self::General,
        Self::Display,
        Self::Graphics,
        Self::Audio,
        Self::Voice,
        Self::Controls,
        Self::Keybindings,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Display => "Display",
            Self::Graphics => "Graphics",
            Self::Audio => "Audio",
            Self::Voice => "Voice",
            Self::Controls => "Controls",
            Self::Keybindings => "Keybindings",
        }
    }
}

/// Pending rebind capture. While `Some(_)` the next physical key press from
/// the player is consumed by the options panel rather than fed to the
/// gameplay input pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingRebind {
    pub(crate) action: KeyAction,
    pub(crate) slot: KeyBindingSlot,
}

#[derive(Resource, Debug, Clone, Copy, Default)]
pub(crate) struct OptionsUiState {
    pub(crate) tab: OptionsTab,
    pub(crate) pending_rebind: Option<PendingRebind>,
}
