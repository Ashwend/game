//! Tracks the flow of replicated world entities arriving from the server so
//! the world-entry readiness gate can wait for the *stream* to finish, not
//! just for the client-side spawn queues to drain.
//!
//! The initial world does not arrive in one burst: the host budgets its own
//! mirror spawns per sync tick (`MAX_RESOURCE_NODE_SPAWNS_PER_SYNC` in
//! `src/net/host/mirror.rs`) and Lightyear paces delivery on top of that, so
//! replicated entities trickle in over seconds. The client's budgeted spawn
//! queues can drain to empty during a lull in that stream, which made a
//! queues-empty readiness check fade the loading splash mid-stream and let
//! the rest of the world pop in on screen. This resource records when the
//! most recent replicated entity arrived; the gate holds until arrivals have
//! been quiet for [`STREAM_QUIET_SECS`].
//!
//! Every per-entity reconciler (resource nodes, deployables, dropped items,
//! loot bags) reports its arrivals here each frame, stamps the connect time,
//! and resets on disconnect; all three calls are idempotent so the writers
//! don't need to coordinate.

use bevy::prelude::Resource;

/// Seconds without a single new replicated entity before the initial stream
/// counts as finished. Mid-stream lulls (server sync-tick pacing, Lightyear
/// send intervals, remote-connection jitter) are far shorter than this;
/// anything longer means the server has nothing left to send for the initial
/// AoI. Bounded overall by the splash's 20 s ready-timeout valve.
pub(crate) const STREAM_QUIET_SECS: f32 = 1.0;

/// If *nothing* has arrived at all, how long after connect the stream still
/// counts as pending. Covers the window between the Welcome and the first
/// replicated entity (room subscription + first paced sync tick); a world
/// with genuinely nothing near spawn stops waiting after this grace.
pub(crate) const STREAM_START_GRACE_SECS: f32 = 2.0;

/// Client-side record of the replicated-entity arrival stream. See the
/// module docs for why this exists.
#[derive(Resource, Default)]
pub(crate) struct WorldStreamState {
    /// `Time::elapsed_secs` at connect, stamped by the first reconciler pass
    /// that runs while a session is live. Cleared on disconnect.
    connected_at_secs: Option<f32>,
    /// `Time::elapsed_secs` of the most recent replicated-entity arrival
    /// (any tracked kind). `None` until the first arrival of a session.
    last_arrival_secs: Option<f32>,
}

impl WorldStreamState {
    /// Stamp the connect time once per session. Idempotent; every reconciler
    /// calls this while connected.
    pub(crate) fn note_connected(&mut self, now_secs: f32) {
        if self.connected_at_secs.is_none() {
            self.connected_at_secs = Some(now_secs);
        }
    }

    /// Record that `count` replicated entities arrived this frame. A zero
    /// count is a no-op so callers can pass their per-frame arrival tally
    /// unconditionally.
    pub(crate) fn note_arrivals(&mut self, now_secs: f32, count: usize) {
        if count > 0 {
            self.last_arrival_secs = Some(now_secs);
        }
    }

    /// Forget the session. Idempotent; every reconciler calls this while
    /// disconnected so the next join starts a fresh stream window.
    pub(crate) fn reset(&mut self) {
        self.connected_at_secs = None;
        self.last_arrival_secs = None;
    }

    /// Whether the initial replication stream has settled: at least
    /// [`STREAM_QUIET_SECS`] since the last arrival, or, if nothing has ever
    /// arrived, [`STREAM_START_GRACE_SECS`] since connect. `false` before any
    /// connected reconciler pass has stamped the session.
    ///
    /// Only meaningful during world entry: in steady-state gameplay the AoI
    /// ring streams entities whenever the player moves, so this flips freely.
    /// The readiness gate consults it only while the loading splash is up.
    pub(crate) fn initial_stream_settled(&self, now_secs: f32) -> bool {
        let Some(connected_at) = self.connected_at_secs else {
            return false;
        };
        match self.last_arrival_secs {
            Some(last) => now_secs - last >= STREAM_QUIET_SECS,
            None => now_secs - connected_at >= STREAM_START_GRACE_SECS,
        }
    }

    /// Seconds since the last arrival, for the stuck-splash diagnostic log.
    pub(crate) fn seconds_since_last_arrival(&self, now_secs: f32) -> Option<f32> {
        self.last_arrival_secs.map(|last| now_secs - last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_settled_before_any_connected_pass() {
        let stream = WorldStreamState::default();
        assert!(!stream.initial_stream_settled(100.0));
    }

    #[test]
    fn arrivals_keep_the_stream_open_until_the_quiet_window_elapses() {
        let mut stream = WorldStreamState::default();
        stream.note_connected(10.0);
        stream.note_arrivals(10.1, 40);
        // Arrivals continue: each one restarts the quiet window.
        stream.note_arrivals(10.6, 8);
        assert!(!stream.initial_stream_settled(10.6 + STREAM_QUIET_SECS * 0.5));
        // A zero-count report must NOT restart the window.
        stream.note_arrivals(11.0, 0);
        assert!(stream.initial_stream_settled(10.6 + STREAM_QUIET_SECS));
        // A late straggler re-opens the stream.
        stream.note_arrivals(12.0, 1);
        assert!(!stream.initial_stream_settled(12.5));
        assert!(stream.initial_stream_settled(12.0 + STREAM_QUIET_SECS));
    }

    #[test]
    fn empty_world_settles_after_the_start_grace() {
        let mut stream = WorldStreamState::default();
        stream.note_connected(5.0);
        assert!(!stream.initial_stream_settled(5.0 + STREAM_START_GRACE_SECS * 0.9));
        assert!(stream.initial_stream_settled(5.0 + STREAM_START_GRACE_SECS));
    }

    #[test]
    fn reset_starts_the_next_session_fresh() {
        let mut stream = WorldStreamState::default();
        stream.note_connected(5.0);
        stream.note_arrivals(5.5, 10);
        stream.reset();
        assert!(
            !stream.initial_stream_settled(100.0),
            "stale session forgotten"
        );
        // Reconnect: connect stamp is fresh, not the old one.
        stream.note_connected(50.0);
        assert!(!stream.initial_stream_settled(50.1));
    }
}
