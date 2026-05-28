//! One-shot per-client overrides for the `multiplayer-test` helper. Each
//! field is sourced from a `GAME_TEST_*` environment variable so the spawn
//! script in `cli/multiplayer_test.rs` can wire up "the test environment"
//! without baking iteration tooling into normal client code paths.
//!
//! Production builds simply see [`TestModeConfig::default`], which is a
//! no-op everywhere.

use bevy::prelude::*;

/// Environment-variable names. Kept in one place so the producer side
/// (`cli/multiplayer_test.rs`) and the consumer side here can't drift.
pub(crate) mod env {
    pub(crate) const WINDOW_WIDTH: &str = "GAME_TEST_WINDOW_WIDTH";
    pub(crate) const WINDOW_HEIGHT: &str = "GAME_TEST_WINDOW_HEIGHT";
    pub(crate) const WINDOW_INDEX: &str = "GAME_TEST_WINDOW_INDEX";
    pub(crate) const WINDOW_COUNT: &str = "GAME_TEST_WINDOW_COUNT";
    pub(crate) const WINDOW_GAP: &str = "GAME_TEST_WINDOW_GAP";
    pub(crate) const SPAWN_OFFSET_X: &str = "GAME_TEST_SPAWN_OFFSET_X";
    pub(crate) const SPAWN_OFFSET_Z: &str = "GAME_TEST_SPAWN_OFFSET_Z";
    pub(crate) const SPAWN_YAW: &str = "GAME_TEST_SPAWN_YAW";
    pub(crate) const INVENTORY_OPEN: &str = "GAME_TEST_INVENTORY_OPEN";
    /// `1` → send `/test-kit` once after the first in-game frame. Used by
    /// the multiplayer-test harness so both clients spawn with the full
    /// tool/resource set (admin gating is also pre-seeded into the save
    /// for the same reason).
    pub(crate) const AUTO_KIT: &str = "GAME_TEST_AUTO_KIT";
}

/// Window-tiling instructions. Stored as "I am window N of M, size W×H,
/// with a G-pixel gap between siblings" — *not* resolved positions —
/// because the only reliable way to centre on the actual display is to
/// query the monitor after Bevy has opened the window. The
/// [`crate::app::systems::reposition_test_window_system`] system does that
/// query and computes the final pixel position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TestWindowLayout {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// 0-based index of this window inside the row.
    pub(crate) index: u32,
    /// Total number of windows the layout is being tiled across. Computed
    /// alongside `index` so a client doesn't need to know about its
    /// siblings to figure out where it belongs.
    pub(crate) count: u32,
    /// Pixel gap between sibling windows.
    pub(crate) gap: i32,
}

#[derive(Resource, Debug, Clone, Default)]
pub(crate) struct TestModeConfig {
    pub(crate) window: Option<TestWindowLayout>,
    /// Meters to add to the server-assigned spawn position once Welcome
    /// arrives. Lets the test helper place two clients on opposite sides
    /// of a center point without any admin teleport plumbing.
    pub(crate) spawn_offset_x: f32,
    pub(crate) spawn_offset_z: f32,
    /// Yaw (radians) to install on the predicted controller after Welcome,
    /// so test clients can be made to face each other from boot. `None`
    /// leaves the server-assigned yaw intact.
    pub(crate) spawn_yaw: Option<f32>,
    /// If true, force the inventory panel open the first time the client
    /// reaches the in-game screen. Useful for the multiplayer-test helper
    /// because the panel is the most common visual surface to debug
    /// against.
    pub(crate) inventory_open_on_join: bool,
    /// If true, fire one `/test-kit` slash command once the client is
    /// in-game. Lets the multiplayer-test helper start both windows
    /// with the full early-game kit (tools, resources, workbench, and
    /// furnace) so PvP / death / crafting paths are immediately
    /// exercisable.
    pub(crate) auto_test_kit_on_join: bool,
}

impl TestModeConfig {
    pub(crate) fn from_env() -> Self {
        let window = match (
            read_env::<u32>(env::WINDOW_WIDTH),
            read_env::<u32>(env::WINDOW_HEIGHT),
            read_env::<u32>(env::WINDOW_INDEX),
            read_env::<u32>(env::WINDOW_COUNT),
        ) {
            (Some(width), Some(height), Some(index), Some(count))
                if width > 0 && height > 0 && count > 0 && index < count =>
            {
                Some(TestWindowLayout {
                    width,
                    height,
                    index,
                    count,
                    gap: read_env::<i32>(env::WINDOW_GAP).unwrap_or(0).max(0),
                })
            }
            _ => None,
        };
        Self {
            window,
            spawn_offset_x: read_env(env::SPAWN_OFFSET_X).unwrap_or(0.0),
            spawn_offset_z: read_env(env::SPAWN_OFFSET_Z).unwrap_or(0.0),
            spawn_yaw: read_env(env::SPAWN_YAW),
            inventory_open_on_join: read_env::<u8>(env::INVENTORY_OPEN).unwrap_or(0) != 0,
            auto_test_kit_on_join: read_env::<u8>(env::AUTO_KIT).unwrap_or(0) != 0,
        }
    }

    /// True when any non-window field would actually change client state.
    /// Lets the apply-once system short-circuit cheaply in production.
    pub(crate) fn has_runtime_overrides(&self) -> bool {
        self.spawn_offset_x != 0.0
            || self.spawn_offset_z != 0.0
            || self.spawn_yaw.is_some()
            || self.inventory_open_on_join
            || self.auto_test_kit_on_join
    }
}

impl TestWindowLayout {
    /// Compute this window's top-left pixel position inside `screen_size`
    /// (logical pixels). Tiles all `count` windows side-by-side, centered
    /// horizontally with `gap` between each pair, and vertically centered.
    pub(crate) fn position_in_screen(self, screen_size: UVec2) -> IVec2 {
        let total_width = (self.width as i32) * (self.count as i32)
            + self.gap * (self.count.saturating_sub(1) as i32);
        let left = ((screen_size.x as i32 - total_width).max(0)) / 2;
        let x = left + (self.index as i32) * ((self.width as i32) + self.gap);
        let y = ((screen_size.y as i32 - self.height as i32).max(0)) / 2;
        IVec2::new(x, y)
    }
}

fn read_env<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok()?.trim().parse::<T>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_no_runtime_overrides() {
        let config = TestModeConfig::default();
        assert!(!config.has_runtime_overrides());
        assert!(config.window.is_none());
    }

    #[test]
    fn position_in_screen_centres_two_window_row() {
        let layout_a = TestWindowLayout {
            width: 880,
            height: 620,
            index: 0,
            count: 2,
            gap: 24,
        };
        let layout_b = TestWindowLayout {
            index: 1,
            ..layout_a
        };
        let screen = UVec2::new(2560, 1440);
        let pos_a = layout_a.position_in_screen(screen);
        let pos_b = layout_b.position_in_screen(screen);

        // Total content width = 880*2 + 24 = 1784, centred in 2560 → left at 388.
        assert_eq!(pos_a.x, 388);
        // Second window is one width + gap to the right of the first.
        assert_eq!(pos_b.x, 388 + 880 + 24);
        // Both y values match — they share a row.
        assert_eq!(pos_a.y, pos_b.y);
        // Vertically centred: (1440 - 620) / 2 = 410.
        assert_eq!(pos_a.y, 410);
    }

    #[test]
    fn position_in_screen_clamps_oversize_layout_to_zero_origin() {
        let layout = TestWindowLayout {
            width: 2000,
            height: 1500,
            index: 0,
            count: 2,
            gap: 24,
        };
        // 4024 wide × 1500 tall on a 1280×720 screen → start at 0,0 for the
        // first window rather than going negative.
        assert_eq!(
            layout.position_in_screen(UVec2::new(1280, 720)),
            IVec2::ZERO
        );
    }
}
