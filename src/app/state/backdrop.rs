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
}
