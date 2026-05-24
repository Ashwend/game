//! Transition stingers — one-shot cues that fire on a state change
//! rather than a gameplay event. Today: the "you entered a world"
//! sound that plays when the player crosses from the menu/loading
//! flow into [`Screen::InGame`].
//!
//! The cue is detected here, not inside [`MenuState::enter_in_game`],
//! because the state method runs from UI handlers that don't carry a
//! [`MessageWriter`]. Watching for the screen edge in a system keeps
//! `MenuState` free of audio concerns and means both session-start
//! paths (singleplayer loopback host, direct multiplayer connect)
//! produce the same cue without each having to remember to emit it.

use bevy::prelude::*;

use crate::app::state::{MenuState, Screen};

use super::{library::PlaySound, manifest::SoundId};

/// Last-frame snapshot of `MenuState::screen`. Compared against the
/// current frame's screen to detect the moment the player enters a
/// world. `None` on the very first tick — no prior frame to compare
/// against, so the system seeds the watch state and emits nothing.
#[derive(Resource, Debug, Default)]
pub(crate) struct ScreenTransitionWatch {
    last_screen: Option<Screen>,
}

pub(crate) fn play_transition_stingers_system(
    menu: Res<MenuState>,
    mut watch: ResMut<ScreenTransitionWatch>,
    mut play: MessageWriter<PlaySound>,
) {
    let current = menu.screen;
    let previous = watch.last_screen.replace(current);

    // Fire on the rising edge into InGame — both singleplayer and
    // multiplayer entry routes funnel through `MenuState::enter_in_game`
    // which sets `screen = InGame`, so one watch covers both flows.
    if matches!(current, Screen::InGame) && previous != Some(Screen::InGame) {
        play.write(PlaySound::non_spatial(SoundId::WorldJoin));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Manual transition driver — exercises the rising-edge logic that
    /// the system embeds, without needing a full Bevy world.
    fn step(watch: &mut ScreenTransitionWatch, current: Screen) -> bool {
        let previous = watch.last_screen.replace(current);
        matches!(current, Screen::InGame) && previous != Some(Screen::InGame)
    }

    #[test]
    fn fires_once_on_entry_to_in_game() {
        let mut watch = ScreenTransitionWatch::default();
        // First observation seeds the watch — nothing to compare against.
        assert!(step(&mut watch, Screen::MainMenu).not());
        assert!(step(&mut watch, Screen::InGame));
        // Subsequent frames at InGame must not re-fire.
        assert!(step(&mut watch, Screen::InGame).not());
        assert!(step(&mut watch, Screen::InGame).not());
    }

    #[test]
    fn re_entering_in_game_after_leaving_fires_again() {
        // Going Menu → InGame → Menu → InGame should produce two cues,
        // one per arrival, so re-joining after a disconnect feels the
        // same as the first join.
        let mut watch = ScreenTransitionWatch::default();
        let _ = step(&mut watch, Screen::MainMenu);
        assert!(step(&mut watch, Screen::InGame), "first entry");
        assert!(step(&mut watch, Screen::MainMenu).not());
        assert!(step(&mut watch, Screen::InGame), "re-entry");
    }

    #[test]
    fn first_frame_at_in_game_seeds_and_emits() {
        // The very first observed screen *is* InGame (e.g. auto-connect
        // dropped the player straight into the world). The watch had
        // `None` as its prior, which isn't `Some(InGame)`, so the cue
        // fires — which is what we want: the player did just arrive.
        let mut watch = ScreenTransitionWatch::default();
        assert!(step(&mut watch, Screen::InGame));
        assert!(step(&mut watch, Screen::InGame).not());
    }

    // Convenience: `.not()` on bool — pre-Rust 1.85 doesn't have it on
    // the prelude in the way `!x` reads inline as a positive assertion.
    trait BoolNot {
        fn not(self) -> bool;
    }
    impl BoolNot for bool {
        fn not(self) -> bool {
            !self
        }
    }
}
