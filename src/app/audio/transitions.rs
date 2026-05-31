//! Transition stingers — one-shot cues that fire on a state change
//! rather than a gameplay event. Today: the "you entered a world"
//! sound that plays when the player crosses from the menu/loading
//! flow into [`Screen::InGame`], reused to score the main-menu reveal
//! when the initial "Authenticating" splash clears at launch.
//!
//! The cue is detected here, not inside [`MenuState::enter_in_game`],
//! because the state method runs from UI handlers that don't carry a
//! [`MessageWriter`]. Watching for the screen edge in a system keeps
//! `MenuState` free of audio concerns and means both session-start
//! paths (singleplayer loopback host, direct multiplayer connect)
//! produce the same cue without each having to remember to emit it.

use bevy::prelude::*;

use crate::app::state::{LoadingSplashKind, MenuState, Screen};

use super::{library::PlaySound, manifest::SoundId};

/// Gain trim for the main-menu reveal cue relative to the in-game world-join.
/// −9 dB ≈ 35% of the linear amplitude — the boot sting is kept gentle so it
/// doesn't greet the player with a blast on launch, while the world-join stays
/// at full weight.
const STARTUP_CUE_GAIN_OFFSET_DB: f32 = -9.0;

/// Last-frame snapshot of the bits of `MenuState` we edge-detect against.
/// Compared each frame to spot the moment the player enters a world and the
/// moment the startup splash clears. Defaults seed the watch on the first
/// tick — no prior frame to compare against, so the system emits nothing.
#[derive(Resource, Debug, Default)]
pub(crate) struct ScreenTransitionWatch {
    last_screen: Option<Screen>,
    /// Whether the startup "Authenticating" splash had begun revealing the menu
    /// (its `ready` flag) last frame.
    startup_splash_ready: bool,
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

    // Reuse the same arrival cue to score the main-menu reveal. Fire it the
    // instant the "Authenticating" splash begins to fade (its `ready` edge),
    // not when it finishes clearing — so the sting rises *with* the menu as it
    // crossfades in, rather than trailing a half-second behind once it's
    // already settled. The startup splash only readies once per launch, so this
    // lands exactly once.
    let startup_splash_ready = menu
        .loading_splash
        .as_ref()
        .is_some_and(|splash| splash.kind == LoadingSplashKind::Startup && splash.ready);
    if reveal_started(watch.startup_splash_ready, startup_splash_ready) {
        play.write(
            PlaySound::non_spatial(SoundId::WorldJoin)
                .with_gain_offset_db(STARTUP_CUE_GAIN_OFFSET_DB),
        );
    }
    watch.startup_splash_ready = startup_splash_ready;
}

/// Rising-edge test for the startup splash beginning its reveal: true exactly
/// on the frame `ready` first flips true.
fn reveal_started(was_ready: bool, is_ready: bool) -> bool {
    is_ready && !was_ready
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

    #[test]
    fn startup_splash_cue_fires_once_when_reveal_begins() {
        // Boot: splash held at full opacity, not yet ready — no cue.
        assert!(reveal_started(false, false).not(), "still authenticating");
        // The frame `ready` flips is the single firing edge — the menu has
        // begun to crossfade in.
        assert!(reveal_started(false, true), "reveal begins");
        // It holds ready through the fade without re-firing…
        assert!(reveal_started(true, true).not(), "mid fade");
        // …and going un-ready (splash dropped) doesn't fire either.
        assert!(reveal_started(true, false).not(), "splash gone");
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
