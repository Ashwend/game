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
    /// Alpha for the opaque cover drawn over the 3D menu backdrop.
    ///
    /// `reveal_allowed` gates the reveal: while it is false (auth still in
    /// flight, outcome unknown) the cover stays fully opaque so the backdrop
    /// never peeks out from behind the loading/login splash. The backdrop keeps
    /// rendering behind the opaque cover, so its blur/DoF converge during the
    /// wait; we pin the warmup timer at its end so the moment the gate lifts the
    /// cover goes straight to the fade instead of re-running the 1.5s warmup
    /// (which would blank the just-revealed menu).
    pub(crate) fn cover_alpha(
        &mut self,
        screen: Screen,
        reveal_allowed: bool,
        delta_seconds: f32,
    ) -> u8 {
        let active = screen.uses_menu_backdrop();
        if active != self.active {
            self.active = active;
            self.elapsed_seconds = 0.0;
        }

        if !active {
            return 0;
        }

        if !reveal_allowed {
            self.elapsed_seconds = MENU_BACKDROP_BLUR_WARMUP_SECONDS;
            return u8::MAX;
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
/// 06:40, early morning: the look chosen for the backdrop by sweeping the dev
/// backdrop-time slider. A low morning sun rakes long soft light across the
/// field, with enough directionality to read as a gradient rather than a hard
/// split, because the grass/prop cel shader floors deep shadow against the real
/// shade rather than crushing it to near-black.
pub(crate) const MENU_BACKDROP_SECONDS: f32 = 6.0 * 3600.0 + 40.0 * 60.0;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_stays_opaque_while_reveal_is_gated() {
        let mut backdrop = MenuBackdropVisibility::default();
        // Far more than the warmup window: with the reveal gated (auth in
        // flight) the cover must never fade, so the 3D backdrop can't peek out
        // behind the loading splash.
        for _ in 0..40 {
            let alpha = backdrop.cover_alpha(Screen::MainMenu, false, 0.1);
            assert_eq!(alpha, u8::MAX, "gated cover must stay fully opaque");
        }
    }

    #[test]
    fn releasing_the_gate_crossfades_without_re_running_the_warmup() {
        let mut backdrop = MenuBackdropVisibility::default();
        // Sit gated for a while (a slow silent restore).
        for _ in 0..20 {
            let _ = backdrop.cover_alpha(Screen::MainMenu, false, 0.1);
        }
        // First frame after the gate lifts is still opaque, then it must fade
        // straight away (no fresh 1.5s warmup) and reach fully transparent
        // within the fade window rather than blanking the revealed menu.
        assert_eq!(backdrop.cover_alpha(Screen::MainMenu, true, 0.0), u8::MAX);
        let mut saw_partial = false;
        let mut revealed = false;
        for _ in 0..30 {
            match backdrop.cover_alpha(Screen::MainMenu, true, 0.05) {
                0 => {
                    revealed = true;
                    break;
                }
                a if a < u8::MAX => saw_partial = true,
                _ => {}
            }
        }
        assert!(saw_partial, "reveal must crossfade, not hard-cut");
        assert!(revealed, "cover must fully clear within the fade window");
    }

    #[test]
    fn ungated_reveal_runs_the_full_warmup_then_fades() {
        // The logged-out path (reveal allowed from the start) keeps the original
        // behaviour: opaque through the warmup, then a fade.
        let mut backdrop = MenuBackdropVisibility::default();
        assert_eq!(backdrop.cover_alpha(Screen::MainMenu, true, 1.0), u8::MAX);
        assert!(!backdrop.has_finished_warmup());
        let _ = backdrop.cover_alpha(Screen::MainMenu, true, 1.0);
        assert!(backdrop.has_finished_warmup());
    }
}
