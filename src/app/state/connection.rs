//! Tracks how long the client has gone without hearing from the server so
//! the HUD can flag a suspected lag/disconnect without needing a dedicated
//! RTT measurement. The server sends `Heartbeat` once per tick on each
//! connected client, so this counter only grows during a real interruption
//! (lossy link, server pause, dropped packets).

/// Threshold (in seconds without a server message) past which the HUD
/// connection indicator switches to a "lagging" state. Server heartbeats
/// land at ~1 Hz, so 2.5s is well outside normal variance.
pub(crate) const CONNECTION_LAG_WARNING_SECONDS: f32 = 2.5;

/// Cap the counter so a long disconnect doesn't accumulate forever — the
/// HUD only cares whether we're past the warning threshold.
const MAX_TRACKED_SILENCE_SECONDS: f32 = 60.0;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ConnectionWatch {
    seconds_since_last_message: f32,
}

impl ConnectionWatch {
    /// A server message just arrived — reset the silence counter.
    pub(crate) fn note_received(&mut self) {
        self.seconds_since_last_message = 0.0;
    }

    /// Advance wall-clock time. When no session is active the counter is
    /// pinned at zero so a freshly-started session doesn't inherit silence
    /// from the previous one.
    pub(crate) fn tick(&mut self, delta_seconds: f32, session_active: bool) {
        if session_active {
            self.seconds_since_last_message = (self.seconds_since_last_message
                + delta_seconds.max(0.0))
            .min(MAX_TRACKED_SILENCE_SECONDS);
        } else {
            self.seconds_since_last_message = 0.0;
        }
    }

    /// Returns true when the session has gone long enough without a server
    /// message that the connection should be flagged as suspect. The
    /// session-active gate is the caller's responsibility — the tracker
    /// only knows about its own counter.
    pub(crate) fn is_lagging(&self, session_active: bool) -> bool {
        session_active && self.seconds_since_last_message >= CONNECTION_LAG_WARNING_SECONDS
    }

    pub(crate) fn reset(&mut self) {
        self.seconds_since_last_message = 0.0;
    }

    #[cfg(test)]
    pub(crate) fn with_silence(seconds: f32) -> Self {
        Self {
            seconds_since_last_message: seconds,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_only_accumulates_while_a_session_is_active() {
        let mut watch = ConnectionWatch::default();
        watch.tick(10.0, false);
        assert!(!watch.is_lagging(false));
        assert!(!watch.is_lagging(true));

        watch.tick(CONNECTION_LAG_WARNING_SECONDS + 1.0, true);
        assert!(watch.is_lagging(true));
        // The flag is still gated on session_active — a session that ends
        // mid-silence should report not-lagging instead of stale-lagging.
        assert!(!watch.is_lagging(false));
    }

    #[test]
    fn note_received_clears_the_silence() {
        let mut watch = ConnectionWatch::default();
        watch.tick(CONNECTION_LAG_WARNING_SECONDS + 0.5, true);
        watch.note_received();
        assert!(!watch.is_lagging(true));
    }

    #[test]
    fn tick_caps_silence_to_keep_long_disconnects_bounded() {
        let mut watch = ConnectionWatch::default();
        watch.tick(MAX_TRACKED_SILENCE_SECONDS * 4.0, true);
        // The is_lagging API is what callers care about — the cap exists
        // purely to keep the internal value finite, not to gate the flag.
        assert!(watch.is_lagging(true));
    }
}
