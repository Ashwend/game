use bevy::prelude::*;

use super::Screen;

pub(super) const MENU_BACKDROP_BLUR_WARMUP_SECONDS: f32 = 1.5;
pub(super) const MENU_BACKDROP_FADE_SECONDS: f32 = 0.5;

#[derive(Resource, Debug, Clone)]
pub(crate) struct MenuBackdropVisibility {
    active: bool,
    elapsed_seconds: f32,
}

impl Default for MenuBackdropVisibility {
    fn default() -> Self {
        Self {
            active: Screen::MainMenu.uses_menu_backdrop(),
            elapsed_seconds: 0.0,
        }
    }
}

impl MenuBackdropVisibility {
    pub(crate) fn cover_alpha(&mut self, screen: Screen, delta_seconds: f32) -> u8 {
        let active = screen.uses_menu_backdrop();
        if active != self.active {
            self.active = active;
            self.elapsed_seconds = 0.0;
        }

        if !active {
            return 0;
        }

        self.elapsed_seconds += delta_seconds.max(0.0);
        if self.elapsed_seconds <= MENU_BACKDROP_BLUR_WARMUP_SECONDS {
            return u8::MAX;
        }

        let fade_progress = ((self.elapsed_seconds - MENU_BACKDROP_BLUR_WARMUP_SECONDS)
            / MENU_BACKDROP_FADE_SECONDS)
            .clamp(0.0, 1.0);
        ((1.0 - fade_progress) * f32::from(u8::MAX)).round() as u8
    }

    /// Returns true once the menu backdrop has been on a backdrop-using
    /// screen long enough to finish its blur warmup. The startup splash
    /// uses this as its readiness signal so the two crossfades, splash
    /// out, backdrop in, happen as a single motion.
    pub(crate) fn has_finished_warmup(&self) -> bool {
        self.active && self.elapsed_seconds >= MENU_BACKDROP_BLUR_WARMUP_SECONDS
    }
}

/// Time of day the menu backdrop's sky is pinned to. The gameplay day/night
/// clock (`ClientRuntime::world_time`) only ticks in-game and keeps the
/// server's last time after you leave a session, so reading it on the title
/// screen makes the backdrop look as if time passed while you were away. The
/// menu renders this fixed time instead, so the title screen is identical on
/// every visit. Pinned, not ticked: the menu never cycles.
///
/// 07:00, early morning: the look chosen for the backdrop by sweeping the dev
/// backdrop-time slider. A low morning sun rakes long soft light across the
/// field, with enough directionality to read as a gradient rather than a hard
/// split, because the grass/prop cel shader floors deep shadow against the real
/// shade rather than crushing it to near-black.
pub(crate) const MENU_BACKDROP_SECONDS: f32 = 7.0 * 3600.0;

/// Live override for the menu-backdrop time of day, read by the sky system when
/// a backdrop-using screen is up. Defaults to the shipped
/// [`MENU_BACKDROP_SECONDS`]; the debug-only title-screen slider
/// (`ui::menu`) mutates it so the backdrop sky can be scrubbed to pick the
/// right pinned time. Release builds never expose the slider, so this stays at
/// the default and the backdrop renders the shipped time.
#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct MenuBackdropTime {
    /// Wall-clock seconds within the in-game day, in `[0, SECONDS_PER_DAY)`.
    pub seconds_of_day: f32,
}

impl Default for MenuBackdropTime {
    fn default() -> Self {
        Self {
            seconds_of_day: MENU_BACKDROP_SECONDS,
        }
    }
}
