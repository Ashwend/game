use crate::world_time::{WorldTime, WorldTimeSnapshot};

use super::GameServer;

impl GameServer {
    pub fn world_time(&self) -> WorldTime {
        self.world_time
    }

    /// Builds a fresh wire snapshot of the day/night clock. Used by both
    /// the routine broadcast and the immediate post-admin-change broadcast.
    pub(crate) fn world_time_snapshot(&self) -> WorldTimeSnapshot {
        WorldTimeSnapshot::from_time(&self.world_time, self.tick)
    }

    /// Admin path: jump the clock to a specific seconds-of-day. Resets the
    /// routine broadcast cadence so the immediate envelope returned by the
    /// caller carries the freshest value.
    pub(crate) fn set_world_time_seconds(&mut self, seconds_of_day: f32) {
        self.world_time.set_seconds(seconds_of_day);
        self.last_world_time_broadcast_tick = self.tick;
    }

    /// Admin path: change the cycle speed. Same routine-broadcast reset as
    /// `set_world_time_seconds` so clients aren't drifting against the
    /// stale multiplier for up to a full broadcast interval.
    pub(crate) fn set_world_time_multiplier(&mut self, multiplier: f32) {
        self.world_time.set_multiplier(multiplier);
        self.last_world_time_broadcast_tick = self.tick;
    }
}
