//! Typed event surface for analytics call sites.
//!
//! Every captured event is one variant of [`Event`]. The associated event
//! name and JSON properties are produced by [`Event::name_and_props`]. New
//! events MUST be added here rather than passed as raw strings, that keeps
//! event names greppable, prevents typos that would silently fork a
//! dashboard, and concentrates the per-property enums (reason categories,
//! screen kinds, etc.) in one file.

use serde_json::{Map, Value, json};

/// Mirror of [`crate::app::state::Screen`] so the analytics module does not
/// pull in `bevy::Resource` or any UI dependency. Mapped at the hook site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenKind {
    MainMenu,
    Options,
    Worlds,
    Multiplayer,
    InGame,
}

impl ScreenKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::MainMenu => "main_menu",
            Self::Options => "options",
            Self::Worlds => "worlds",
            Self::Multiplayer => "multiplayer",
            Self::InGame => "in_game",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionMode {
    Singleplayer,
    Multiplayer,
}

impl SessionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Singleplayer => "singleplayer",
            Self::Multiplayer => "multiplayer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionEndReason {
    UserQuit,
    Kick,
    Disconnect,
}

impl SessionEndReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserQuit => "user_quit",
            Self::Kick => "kick",
            Self::Disconnect => "disconnect",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectFailReason {
    BadAddress,
    Timeout,
    Refused,
    VersionMismatch,
    AuthRejected,
    Other,
}

impl ConnectFailReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::BadAddress => "bad_address",
            Self::Timeout => "timeout",
            Self::Refused => "refused",
            Self::VersionMismatch => "version_mismatch",
            Self::AuthRejected => "auth_rejected",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorCategory {
    Network,
    Save,
    Auth,
    Protocol,
}

impl ErrorCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Save => "save",
            Self::Auth => "auth",
            Self::Protocol => "protocol",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    AppStarted,
    AppQuit {
        duration_s: f64,
    },
    ScreenViewed {
        screen: ScreenKind,
    },
    WorldCreated {
        map_type: String,
    },
    WorldLoaded,
    WorldDeleted,
    SessionStarted {
        mode: SessionMode,
    },
    SessionEnded {
        duration_s: f64,
        reason: SessionEndReason,
    },
    ConnectAttempted {
        target_host_masked: String,
    },
    ConnectSucceeded,
    ConnectFailed {
        reason: ConnectFailReason,
    },
    DeployablePlaced {
        kind: String,
    },
    Error {
        category: ErrorCategory,
    },
    /// Worker-internal: surfaced when the channel overflows so dashboards
    /// can spot dropped traffic without needing log access.
    AnalyticsDropped {
        count: u64,
    },
}

impl Event {
    pub(crate) fn name_and_props(&self) -> (&'static str, Map<String, Value>) {
        match self {
            Self::AppStarted => ("app_started", Map::new()),
            Self::AppQuit { duration_s } => (
                "app_quit",
                props(&[("duration_s", json!(round2(*duration_s)))]),
            ),
            Self::ScreenViewed { screen } => (
                "screen_viewed",
                props(&[("screen", json!(screen.as_str()))]),
            ),
            Self::WorldCreated { map_type } => {
                ("world_created", props(&[("map_type", json!(map_type))]))
            }
            Self::WorldLoaded => ("world_loaded", Map::new()),
            Self::WorldDeleted => ("world_deleted", Map::new()),
            Self::SessionStarted { mode } => {
                ("session_started", props(&[("mode", json!(mode.as_str()))]))
            }
            Self::SessionEnded { duration_s, reason } => (
                "session_ended",
                props(&[
                    ("duration_s", json!(round2(*duration_s))),
                    ("reason", json!(reason.as_str())),
                ]),
            ),
            Self::ConnectAttempted { target_host_masked } => (
                "connect_attempted",
                props(&[("target_host_masked", json!(target_host_masked))]),
            ),
            Self::ConnectSucceeded => ("connect_succeeded", Map::new()),
            Self::ConnectFailed { reason } => (
                "connect_failed",
                props(&[("reason", json!(reason.as_str()))]),
            ),
            Self::DeployablePlaced { kind } => {
                ("deployable_placed", props(&[("kind", json!(kind))]))
            }
            Self::Error { category } => ("error", props(&[("category", json!(category.as_str()))])),
            Self::AnalyticsDropped { count } => {
                ("analytics_dropped", props(&[("count", json!(*count))]))
            }
        }
    }
}

fn props(entries: &[(&str, Value)]) -> Map<String, Value> {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert((*key).to_owned(), value.clone());
    }
    map
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Best-effort masking for a user-typed host used in connect events.
/// Returns one of: `"localhost"`, `"ip_literal"`, the registered domain
/// (e.g. `example.com` from `play.example.com:7777`), or `"unknown"`.
/// Never includes the port or full IP.
pub(crate) fn mask_host(input: &str) -> String {
    let host = input
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("");
    // Strip an `:port` suffix if present. IPv6 literals like `[::1]:7777`
    // are uncommon enough that we just bucket them as `ip_literal` below;
    // a stray bracket survives into the IP-literal check.
    let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    let host = host.trim_matches(|c| c == '[' || c == ']');
    if host.is_empty() {
        return "unknown".to_owned();
    }
    if host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1" {
        return "localhost".to_owned();
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return "ip_literal".to_owned();
    }

    // Registered-domain heuristic without a public-suffix list: keep the
    // last two labels for plain TLDs (`example.com`), three for a known
    // two-part suffix (`co.uk`, `co.jp`, `com.au`, `org.uk`).
    let labels: Vec<&str> = host.split('.').filter(|label| !label.is_empty()).collect();
    if labels.is_empty() {
        return "unknown".to_owned();
    }
    if labels.len() == 1 {
        return labels[0].to_ascii_lowercase();
    }
    let take = if labels.len() >= 3
        && is_two_part_suffix(labels[labels.len() - 2], labels[labels.len() - 1])
    {
        3
    } else {
        2
    };
    labels[labels.len() - take..].join(".").to_ascii_lowercase()
}

fn is_two_part_suffix(second_last: &str, last: &str) -> bool {
    matches!(
        (
            second_last.to_ascii_lowercase().as_str(),
            last.to_ascii_lowercase().as_str()
        ),
        ("co", "uk")
            | ("co", "jp")
            | ("co", "nz")
            | ("com", "au")
            | ("com", "br")
            | ("org", "uk")
            | ("gov", "uk")
            | ("ac", "uk"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(event: Event) -> serde_json::Value {
        let (name, props) = event.name_and_props();
        serde_json::json!({ "event": name, "properties": Value::Object(props) })
    }

    #[test]
    fn app_started_serializes_with_empty_props() {
        let value = shape(Event::AppStarted);
        assert_eq!(value["event"], "app_started");
        assert_eq!(value["properties"], json!({}));
    }

    #[test]
    fn session_ended_carries_duration_and_reason() {
        let value = shape(Event::SessionEnded {
            duration_s: 123.456,
            reason: SessionEndReason::Disconnect,
        });
        assert_eq!(value["event"], "session_ended");
        assert_eq!(value["properties"]["duration_s"], json!(123.46));
        assert_eq!(value["properties"]["reason"], "disconnect");
    }

    #[test]
    fn connect_failed_uses_short_reason_strings() {
        for (reason, expected) in [
            (ConnectFailReason::BadAddress, "bad_address"),
            (ConnectFailReason::Timeout, "timeout"),
            (ConnectFailReason::Refused, "refused"),
            (ConnectFailReason::VersionMismatch, "version_mismatch"),
            (ConnectFailReason::AuthRejected, "auth_rejected"),
            (ConnectFailReason::Other, "other"),
        ] {
            let value = shape(Event::ConnectFailed { reason });
            assert_eq!(value["event"], "connect_failed");
            assert_eq!(value["properties"]["reason"], expected);
        }
    }

    #[test]
    fn mask_host_buckets_localhost_and_ip_literals() {
        assert_eq!(mask_host("localhost"), "localhost");
        assert_eq!(mask_host("127.0.0.1"), "localhost");
        assert_eq!(mask_host("127.0.0.1:7777"), "localhost");
        assert_eq!(mask_host("192.168.1.20"), "ip_literal");
        assert_eq!(mask_host("8.8.8.8:7777"), "ip_literal");
    }

    #[test]
    fn mask_host_strips_subdomain_and_port_for_normal_domain() {
        assert_eq!(mask_host("play.example.com:7777"), "example.com");
        assert_eq!(mask_host("https://play.example.com"), "example.com");
        assert_eq!(mask_host("EXAMPLE.COM"), "example.com");
    }

    #[test]
    fn mask_host_preserves_two_part_tld() {
        assert_eq!(mask_host("play.example.co.uk:7777"), "example.co.uk");
        assert_eq!(mask_host("subdomain.foo.com.au"), "foo.com.au");
    }

    #[test]
    fn mask_host_handles_garbage() {
        assert_eq!(mask_host(""), "unknown");
        assert_eq!(mask_host("   "), "unknown");
    }
}
