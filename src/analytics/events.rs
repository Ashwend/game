//! Typed event surface for analytics call sites.
//!
//! Every captured event is one variant of [`Event`]. The associated event
//! name and JSON properties are produced by [`Event::name_and_props`]. New
//! events MUST be added here rather than passed as raw strings, that keeps
//! event names greppable, prevents typos that would silently fork a
//! dashboard, and concentrates the per-property enums (reason categories,
//! screen kinds, etc.) in one file.

use serde_json::{Map, Value, json};

/// Mirror of `crate::app::state::Screen` so the analytics module does not
/// pull in `bevy::Resource` or any UI dependency, plus `SignIn` for the
/// pre-auth login splash, which is gated by `AuthFlow` (not a `Screen` variant)
/// and so isn't covered by `map_screen`. Mapped at the hook site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenKind {
    SignIn,
    MainMenu,
    Options,
    Worlds,
    Multiplayer,
    InGame,
}

impl ScreenKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::SignIn => "sign_in",
            Self::MainMenu => "main_menu",
            Self::Options => "options",
            Self::Worlds => "worlds",
            Self::Multiplayer => "multiplayer",
            Self::InGame => "in_game",
        }
    }
}

/// Which login-screen button started a sign-in, so the funnel can tell
/// returning sign-ins from new-account creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthMethod {
    SignIn,
    CreateAccount,
}

impl AuthMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::SignIn => "sign_in",
            Self::CreateAccount => "create_account",
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

/// Coarse cause of a local death, taken from what the `PlayerKilled` wire
/// message cheaply carries: a named killer (another player) versus none
/// (the environment, the meteor shower, or a self-inflicted explosive).
/// The exact `DamageKind` is not on that message, and adding it would be a
/// protocol change out of scope for the analytics pass, so this stays coarse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeathCause {
    Player,
    Environment,
}

impl DeathCause {
    fn as_str(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Environment => "environment",
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
    /// The user clicked "Sign in" / "Create account" on the login splash,
    /// kicking off the browser round-trip.
    SignInStarted {
        method: AuthMethod,
    },
    /// The user signed out from the title screen (the confirmed sign-out
    /// action, not a session that lapsed on its own).
    SignedOut,
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
    /// A crafting job finished. `recipe_id` is the registry id string (never a
    /// display name), matching the id shipped in `CraftingCommand::Enqueue`.
    CraftCompleted {
        recipe_id: String,
    },
    /// A piece of armor was moved into an equipment slot. `item_id` is the
    /// registry id; `slot` is the worn slot ("head" / "chest" / "legs" /
    /// "feet").
    ItemEquipped {
        item_id: String,
        slot: String,
    },
    /// A placed workbench was upgraded to a higher tier. `tier` is the new
    /// (post-upgrade) tier.
    WorkbenchUpgraded {
        tier: u8,
    },
    /// The local player loosed a shot from a ranged weapon. `weapon` is the
    /// registry id (`wooden_bow` / `crossbow`). Sampled, not one-per-shot, so a
    /// held crossbow trigger does not flood the pipe (see the fire site).
    RangedFired {
        weapon: String,
    },
    /// An explosive detonated within cue range of the local player, observed off
    /// the cosmetic `ServerMessage::Explosion`. `kind` is the charge kind.
    ExplosiveDetonated {
        kind: String,
    },
    /// The local player defused a placed charge. `kind` is the charge kind.
    ExplosiveDefused {
        kind: String,
    },
    /// The client received the meteor shower announce (a meteor event went live).
    MeteorShowerAnnounced,
    /// The local player was within cue range of a meteor shower impact when it
    /// struck (they witnessed the strike, whether or not it hit them).
    MeteorShowerImpactWitnessed,
    /// The local player died. `cause` is coarse (player kill vs environment);
    /// see [`DeathCause`].
    PlayerDeath {
        cause: DeathCause,
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
            Self::SignInStarted { method } => (
                "sign_in_started",
                props(&[("method", json!(method.as_str()))]),
            ),
            Self::SignedOut => ("signed_out", Map::new()),
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
            Self::CraftCompleted { recipe_id } => {
                ("craft_completed", props(&[("recipe_id", json!(recipe_id))]))
            }
            Self::ItemEquipped { item_id, slot } => (
                "item_equipped",
                props(&[("item_id", json!(item_id)), ("slot", json!(slot))]),
            ),
            Self::WorkbenchUpgraded { tier } => {
                ("workbench_upgraded", props(&[("tier", json!(*tier))]))
            }
            Self::RangedFired { weapon } => ("ranged_fired", props(&[("weapon", json!(weapon))])),
            Self::ExplosiveDetonated { kind } => {
                ("explosive_detonated", props(&[("kind", json!(kind))]))
            }
            Self::ExplosiveDefused { kind } => {
                ("explosive_defused", props(&[("kind", json!(kind))]))
            }
            Self::MeteorShowerAnnounced => ("meteor_shower_announced", Map::new()),
            Self::MeteorShowerImpactWitnessed => ("meteor_shower_impact_witnessed", Map::new()),
            Self::PlayerDeath { cause } => {
                ("player_death", props(&[("cause", json!(cause.as_str()))]))
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
        ("co" | "org" | "gov" | "ac", "uk") | ("co", "jp" | "nz") | ("com", "au" | "br"),
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
    fn sign_in_started_carries_method() {
        let value = shape(Event::SignInStarted {
            method: AuthMethod::CreateAccount,
        });
        assert_eq!(value["event"], "sign_in_started");
        assert_eq!(value["properties"]["method"], "create_account");

        let value = shape(Event::SignInStarted {
            method: AuthMethod::SignIn,
        });
        assert_eq!(value["properties"]["method"], "sign_in");
    }

    #[test]
    fn signed_out_serializes_with_empty_props() {
        let value = shape(Event::SignedOut);
        assert_eq!(value["event"], "signed_out");
        assert_eq!(value["properties"], json!({}));
    }

    #[test]
    fn sign_in_screen_maps_to_snake_case() {
        let value = shape(Event::ScreenViewed {
            screen: ScreenKind::SignIn,
        });
        assert_eq!(value["event"], "screen_viewed");
        assert_eq!(value["properties"]["screen"], "sign_in");
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
    fn craft_completed_carries_recipe_id() {
        let value = shape(Event::CraftCompleted {
            recipe_id: "iron_sword".to_owned(),
        });
        assert_eq!(value["event"], "craft_completed");
        assert_eq!(value["properties"]["recipe_id"], "iron_sword");
    }

    #[test]
    fn item_equipped_carries_item_and_slot() {
        let value = shape(Event::ItemEquipped {
            item_id: "iron_chest".to_owned(),
            slot: "chest".to_owned(),
        });
        assert_eq!(value["event"], "item_equipped");
        assert_eq!(value["properties"]["item_id"], "iron_chest");
        assert_eq!(value["properties"]["slot"], "chest");
    }

    #[test]
    fn workbench_upgraded_carries_tier_as_number() {
        let value = shape(Event::WorkbenchUpgraded { tier: 2 });
        assert_eq!(value["event"], "workbench_upgraded");
        assert_eq!(value["properties"]["tier"], json!(2));
    }

    #[test]
    fn ranged_fired_carries_weapon() {
        let value = shape(Event::RangedFired {
            weapon: "crossbow".to_owned(),
        });
        assert_eq!(value["event"], "ranged_fired");
        assert_eq!(value["properties"]["weapon"], "crossbow");
    }

    #[test]
    fn explosive_events_carry_kind() {
        let detonated = shape(Event::ExplosiveDetonated {
            kind: "powder_keg".to_owned(),
        });
        assert_eq!(detonated["event"], "explosive_detonated");
        assert_eq!(detonated["properties"]["kind"], "powder_keg");

        let defused = shape(Event::ExplosiveDefused {
            kind: "satchel_charge".to_owned(),
        });
        assert_eq!(defused["event"], "explosive_defused");
        assert_eq!(defused["properties"]["kind"], "satchel_charge");
    }

    #[test]
    fn meteor_shower_events_have_empty_props() {
        assert_eq!(shape(Event::MeteorShowerAnnounced)["properties"], json!({}));
        assert_eq!(
            shape(Event::MeteorShowerImpactWitnessed)["properties"],
            json!({})
        );
    }

    #[test]
    fn player_death_records_coarse_cause() {
        let by_player = shape(Event::PlayerDeath {
            cause: DeathCause::Player,
        });
        assert_eq!(by_player["event"], "player_death");
        assert_eq!(by_player["properties"]["cause"], "player");

        let by_env = shape(Event::PlayerDeath {
            cause: DeathCause::Environment,
        });
        assert_eq!(by_env["properties"]["cause"], "environment");
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
