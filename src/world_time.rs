//! Day/night cycle clock shared between server and client.
//!
//! The server owns authoritative time. It advances `seconds_of_day` every
//! tick by `delta * multiplier` and ships periodic [`WorldTimeSnapshot`]
//! messages to clients. Clients integrate their own copy between snapshots
//! using the same multiplier so the visible sun/moon position stays smooth
//! even when packets arrive 60 s apart.
//!
//! Time units: `seconds_of_day` is wall-clock seconds in `[0, SECONDS_PER_DAY)`.
//! At the default `multiplier = 1.0`, a full 24 h cycle spans `SECONDS_PER_DAY`
//! real seconds (30 min), so one in-game hour is 75 real seconds.

use serde::{Deserialize, Serialize};

/// Real-world seconds it takes the in-game clock to traverse a full
/// 24 h day at `multiplier = 1.0`. 30 min by design — short enough that
/// a single play session lands across several cycles.
pub const REAL_SECONDS_PER_DAY: f32 = 30.0 * 60.0;

/// Length of the in-game day in "in-game seconds" used by the cycle math.
/// We keep one in-game second equal to one tick of `seconds_of_day` so the
/// scaling math stays in one place: `delta_seconds_in_game = delta_real *
/// SECONDS_PER_DAY / REAL_SECONDS_PER_DAY * multiplier`.
pub const SECONDS_PER_DAY: f32 = 24.0 * 60.0 * 60.0;

/// The fixed real→in-game scale at `multiplier = 1.0`. Pre-computed so the
/// per-tick advance is a single mul rather than two divs.
pub const REAL_TO_IN_GAME: f32 = SECONDS_PER_DAY / REAL_SECONDS_PER_DAY;

/// Hardcoded ceiling on the speed multiplier. 240× = a full cycle in ~7.5 s
/// real time, fast enough for an admin to flick through sunrise/sunset.
pub const MAX_MULTIPLIER: f32 = 240.0;
/// Hardcoded floor. Pausing time (`0.0`) is allowed; negative is not — we
/// don't reverse the cycle because shadow/light interpolation assumes
/// monotonic advance within a snapshot window.
pub const MIN_MULTIPLIER: f32 = 0.0;

/// Initial wall-clock time when a brand-new world is created. 07:00 — just
/// after sunrise so the player spawns into daylight without a black screen.
pub const DEFAULT_START_SECONDS: f32 = 7.0 * 3600.0;

/// Authoritative server clock. Mirrored on the client via
/// [`WorldTimeSnapshot`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WorldTime {
    /// Wall-clock seconds within the in-game day, in `[0, SECONDS_PER_DAY)`.
    pub seconds_of_day: f32,
    /// Real-to-in-game speed multiplier. `1.0` ticks one in-game day per
    /// `REAL_SECONDS_PER_DAY` real seconds. `0.0` pauses the cycle.
    pub multiplier: f32,
}

impl Default for WorldTime {
    fn default() -> Self {
        Self {
            seconds_of_day: DEFAULT_START_SECONDS,
            multiplier: 1.0,
        }
    }
}

impl WorldTime {
    /// Advance the clock by `delta_real_seconds` of real time. Wraps at
    /// midnight so callers never have to remember to `rem_euclid`.
    pub fn advance(&mut self, delta_real_seconds: f32) {
        if !delta_real_seconds.is_finite() || delta_real_seconds <= 0.0 {
            return;
        }
        if !self.multiplier.is_finite() || self.multiplier <= 0.0 {
            return;
        }
        let advance = delta_real_seconds * REAL_TO_IN_GAME * self.multiplier;
        self.seconds_of_day = (self.seconds_of_day + advance).rem_euclid(SECONDS_PER_DAY);
    }

    /// Force the wall-clock time, normalising into `[0, SECONDS_PER_DAY)`.
    pub fn set_seconds(&mut self, seconds_of_day: f32) {
        if !seconds_of_day.is_finite() {
            return;
        }
        self.seconds_of_day = seconds_of_day.rem_euclid(SECONDS_PER_DAY);
    }

    /// Clamp the speed multiplier into `[MIN_MULTIPLIER, MAX_MULTIPLIER]`.
    pub fn set_multiplier(&mut self, multiplier: f32) {
        if !multiplier.is_finite() {
            return;
        }
        self.multiplier = multiplier.clamp(MIN_MULTIPLIER, MAX_MULTIPLIER);
    }

    /// Position in the day as a `[0, 1)` fraction. Convenient for cycle
    /// math that doesn't care about the underlying second count.
    pub fn day_fraction(&self) -> f32 {
        self.seconds_of_day / SECONDS_PER_DAY
    }

    /// `HH:MM` human-readable format. Used by admin command echoes.
    pub fn format_hhmm(&self) -> String {
        let total = self.seconds_of_day.max(0.0) as u32;
        let hours = (total / 3600) % 24;
        let minutes = (total / 60) % 60;
        format!("{hours:02}:{minutes:02}")
    }
}

/// Wire payload for the periodic time broadcast. Mirrors the live
/// [`WorldTime`] plus the server tick the value was sampled on so the
/// client can re-align without trusting wall-clock skew.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WorldTimeSnapshot {
    pub seconds_of_day: f32,
    pub multiplier: f32,
    pub server_tick: u64,
}

impl WorldTimeSnapshot {
    pub fn from_time(time: &WorldTime, server_tick: u64) -> Self {
        Self {
            seconds_of_day: time.seconds_of_day,
            multiplier: time.multiplier,
            server_tick,
        }
    }
}

/// Parse a player-supplied time token. Accepts, in order:
/// - `HH:MM` (`06:30`),
/// - `HHMM` military time with no colon (`0700`, `1430`) — any 3-4 digit
///   integer, so a leading zero isn't required,
/// - a bare hour (`14`, `7.5`), wrapped into the day.
///
/// Returns the normalised seconds-of-day.
pub fn parse_time_token(token: &str) -> Option<f32> {
    let token = token.trim();
    if let Some((h, m)) = token.split_once(':') {
        return hhmm_seconds(h.parse().ok()?, m.parse().ok()?);
    }

    // `HHMM` with no colon. A 3-4 digit integer almost certainly means a clock
    // time (`0700` = 07:00), not "700 hours" — which the bare-hour branch below
    // would silently wrap to a nonsensical time of day. Split off the trailing
    // two digits as minutes.
    if (3..=4).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_digit()) {
        let value: u32 = token.parse().ok()?;
        return hhmm_seconds(value / 100, value % 100);
    }

    if let Ok(hours) = token.parse::<f32>()
        && hours.is_finite()
    {
        let seconds = hours * 3600.0;
        return Some(seconds.rem_euclid(SECONDS_PER_DAY));
    }
    None
}

/// Seconds-of-day for an `HH`/`MM` pair, or `None` if either is out of range.
fn hhmm_seconds(hours: u32, minutes: u32) -> Option<f32> {
    if hours >= 24 || minutes >= 60 {
        return None;
    }
    Some((hours * 3600 + minutes * 60) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_wraps_around_midnight() {
        let mut time = WorldTime {
            seconds_of_day: SECONDS_PER_DAY - 10.0,
            multiplier: 1.0,
        };
        let real_seconds_for_20s_ingame = 20.0 / REAL_TO_IN_GAME;
        time.advance(real_seconds_for_20s_ingame);
        assert!(time.seconds_of_day < 20.0);
    }

    #[test]
    fn negative_or_zero_multiplier_does_not_advance() {
        let mut time = WorldTime {
            seconds_of_day: 0.0,
            multiplier: 0.0,
        };
        time.advance(10.0);
        assert_eq!(time.seconds_of_day, 0.0);
    }

    #[test]
    fn set_multiplier_clamps_to_safe_range() {
        let mut time = WorldTime::default();
        time.set_multiplier(-5.0);
        assert_eq!(time.multiplier, MIN_MULTIPLIER);
        time.set_multiplier(10_000.0);
        assert_eq!(time.multiplier, MAX_MULTIPLIER);
    }

    #[test]
    fn parse_time_token_handles_hh_mm_and_hours() {
        assert!((parse_time_token("06:30").unwrap() - 23_400.0).abs() < 0.01);
        assert!((parse_time_token("23").unwrap() - 82_800.0).abs() < 0.01);
        assert!(parse_time_token("nope").is_none());
        assert!(parse_time_token("25:00").is_none());
        assert!(parse_time_token("12:60").is_none());
    }

    #[test]
    fn parse_time_token_handles_hhmm_without_colon() {
        // `0700` is 07:00, not "700 hours" wrapped to 04:00 (the old bug).
        assert!((parse_time_token("0700").unwrap() - 25_200.0).abs() < 0.01);
        assert!((parse_time_token("0800").unwrap() - 28_800.0).abs() < 0.01);
        assert!((parse_time_token("1430").unwrap() - 52_200.0).abs() < 0.01);
        // 3-digit form: leading zero optional.
        assert!((parse_time_token("800").unwrap() - 28_800.0).abs() < 0.01);
        assert!((parse_time_token("100").unwrap() - 3_600.0).abs() < 0.01);
        // Out-of-range clock components are rejected, not wrapped.
        assert!(parse_time_token("2400").is_none());
        assert!(parse_time_token("1260").is_none());
        // 1-2 digit tokens stay "bare hour" (07:00, not split as H:M).
        assert!((parse_time_token("7").unwrap() - 25_200.0).abs() < 0.01);
    }

    #[test]
    fn format_hhmm_is_zero_padded() {
        let time = WorldTime {
            seconds_of_day: 6.0 * 3600.0 + 5.0 * 60.0,
            multiplier: 1.0,
        };
        assert_eq!(time.format_hhmm(), "06:05");
    }

    #[test]
    fn default_time_starts_in_the_morning() {
        let time = WorldTime::default();
        assert!(time.seconds_of_day > 6.0 * 3600.0);
        assert!(time.seconds_of_day < 8.0 * 3600.0);
        assert_eq!(time.multiplier, 1.0);
    }
}
