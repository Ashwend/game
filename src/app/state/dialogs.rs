use std::{
    net::SocketAddr,
    sync::{Mutex, mpsc::Receiver},
};

use uuid::Uuid;

use crate::{
    net::ClientSession,
    save::WorldSummary,
    world::{MapType, ProceduralMapSize},
};

#[derive(Debug, Clone)]
pub(crate) struct ConfirmationDialog {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) confirm_label: String,
    pub(crate) cancel_label: String,
    pub(crate) action: ConfirmationAction,
    pub(crate) closing: bool,
    pub(crate) confirmed: bool,
}

impl ConfirmationDialog {
    pub(crate) fn delete_world(world_id: Uuid, world_name: &str) -> Self {
        Self {
            title: "Delete World".to_owned(),
            body: format!("Permanently delete \"{world_name}\"? This cannot be undone."),
            confirm_label: "Delete".to_owned(),
            cancel_label: "Cancel".to_owned(),
            action: ConfirmationAction::DeleteWorld { world_id },
            closing: false,
            confirmed: false,
        }
    }

    /// Guard the sign-out link so a stray click can't drop the player out of
    /// the only identity system.
    pub(crate) fn sign_out() -> Self {
        Self {
            title: "Sign out".to_owned(),
            body: "Sign out of Ashwend? You'll need to sign in again to play.".to_owned(),
            confirm_label: "Sign out".to_owned(),
            cancel_label: "Cancel".to_owned(),
            action: ConfirmationAction::SignOut,
            closing: false,
            confirmed: false,
        }
    }

    /// Guard a world-map marker deletion behind the shared confirm modal.
    pub(crate) fn delete_world_map_marker(id: u32, name: &str) -> Self {
        let body = if name.is_empty() {
            "Delete this map marker?".to_owned()
        } else {
            format!("Delete the marker \"{name}\"?")
        };
        Self {
            title: "Delete Marker".to_owned(),
            body,
            confirm_label: "Delete".to_owned(),
            cancel_label: "Cancel".to_owned(),
            action: ConfirmationAction::DeleteWorldMapMarker { id },
            closing: false,
            confirmed: false,
        }
    }

    /// Guard the options Reset button: it wipes every tab, including
    /// keybindings, so confirm before discarding the player's setup.
    pub(crate) fn reset_settings() -> Self {
        Self {
            title: "Reset settings".to_owned(),
            body: "Reset all settings to their defaults? This affects every tab, including your keybindings.".to_owned(),
            confirm_label: "Reset all".to_owned(),
            cancel_label: "Cancel".to_owned(),
            action: ConfirmationAction::ResetSettings,
            closing: false,
            confirmed: false,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ConfirmationAction {
    DeleteWorld {
        world_id: Uuid,
    },
    /// Delete one of the player's own world-map markers (server-side). The
    /// confirm handler can't reach the network, so it arms
    /// `MenuState::world_map_delete_pending` and `world_map_input_system`
    /// sends the command.
    DeleteWorldMapMarker {
        id: u32,
    },
    SignOut,
    ResetSettings,
}

#[derive(Debug, Clone)]
pub(crate) struct NoticeDialog {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) confirm_label: String,
    pub(crate) closing: bool,
}

impl NoticeDialog {
    pub(crate) fn disconnected(reason: impl Into<String>) -> Self {
        Self {
            title: "Disconnected".to_owned(),
            body: reason.into(),
            confirm_label: "OK".to_owned(),
            closing: false,
        }
    }

    /// Notice for a client/server version mismatch. Shows both versions and
    /// tells the player whether their build is older or newer than the server,
    /// so they know which side needs updating. `client_version` is the local
    /// build (the caller passes `GAME_VERSION`); `server_version` is what the
    /// server reported over the handshake.
    pub(crate) fn version_mismatch(client_version: &str, server_version: &str) -> Self {
        let hint = match compare_versions(client_version, server_version) {
            Some(std::cmp::Ordering::Less) => {
                "Your game is older than the server. Update to the latest version to play."
            }
            Some(std::cmp::Ordering::Greater) => {
                "Your game is newer than the server, which hasn't been updated yet."
            }
            // Equal protocol numbers can still mismatch (different build with
            // the same parsed version), and unparseable versions fall here too.
            _ => "Your game doesn't match the server. Make sure you're on the latest version.",
        };
        Self {
            title: "Version mismatch".to_owned(),
            body: format!(
                "This server is running a different version of Ashwend.\n\n\
                 Your version:\u{2002}\u{2002}{client_version}\n\
                 Server version:\u{2002}{server_version}\n\n\
                 {hint}"
            ),
            confirm_label: "OK".to_owned(),
            closing: false,
        }
    }

    /// Friendly notice for a connection the server refused at the handshake
    /// (bad/expired auth ticket, or a generic auth failure). Version
    /// mismatches use [`Self::version_mismatch`] instead. The raw `reason`
    /// from the server is shown when present so the player has something
    /// actionable, but it's framed so an empty reason still reads cleanly.
    pub(crate) fn auth_rejected(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        // An expired/invalid signature is the common case (the access token
        // lapsed during a long session) and has a clear remedy, so it gets its
        // own headline instead of a generic "couldn't verify" line. The raw
        // reason still rides along as a detail for bug reports.
        let lower = reason.to_ascii_lowercase();
        let expired = lower.contains("expired") || lower.contains("signature");
        let headline = if expired {
            "Your sign-in session expired and couldn't be verified. Sign out and back in, then \
             rejoin."
        } else {
            "The server refused the connection. Your login couldn't be verified."
        };
        let body = if reason.trim().is_empty() {
            headline.to_owned()
        } else {
            format!("{headline}\n\nDetails: {reason}")
        };
        Self {
            title: "Couldn't join server".to_owned(),
            body,
            confirm_label: "OK".to_owned(),
            closing: false,
        }
    }

    /// Generic error notice: a failure the player must acknowledge rather
    /// than a transient status line they can miss (failed world create or
    /// load, failed delete, failed rename). `title` names the action that
    /// failed in plain words ("Couldn't create world"); `body` carries the
    /// underlying error text.
    pub(crate) fn error(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            confirm_label: "OK".to_owned(),
            closing: false,
        }
    }
}

/// Order `a` against `b` as dotted version strings (`"0.14.1"`). Compares
/// major, then minor, then patch numerically. Returns `None` if either side
/// can't be parsed, so the caller can fall back to a neutral message rather
/// than guessing a direction.
fn compare_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    Some(parse_version(a)?.cmp(&parse_version(b)?))
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().split('.');
    let major = leading_number(parts.next()?)?;
    let minor = leading_number(parts.next().unwrap_or("0"))?;
    let patch = leading_number(parts.next().unwrap_or("0"))?;
    Some((major, minor, patch))
}

/// Parse the leading run of ASCII digits (so a pre-release suffix like
/// `"1-beta"` still yields `1`). `None` when there are no leading digits.
fn leading_number(segment: &str) -> Option<u64> {
    let digits: String = segment.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Why a multiplayer join attempt failed, beyond a plain error string. The
/// connect worker classifies the failure so the UI can route it correctly: a
/// session that can't be renewed logs the player out, a renewal that failed
/// lets them retry, and everything else is a normal connection error.
pub(crate) enum JoinError {
    /// Resolve / handshake / transport failure. The raw text is classified and
    /// surfaced the same way connection errors always have been.
    Connection(String),
    /// The access token was expired/absent and no refresh token is stored to
    /// renew it from. The player has to sign in again.
    SignInRequired,
    /// A refresh token existed but renewing it failed (network/provider error).
    /// The player can retry the join; the underlying error rides along.
    RenewFailed(String),
}

pub(crate) type DirectConnectResult = std::result::Result<(SocketAddr, ClientSession), JoinError>;

pub(crate) struct DirectConnectAttempt {
    pub(crate) receiver: Mutex<Receiver<DirectConnectResult>>,
}

pub(crate) type WorldStartResult = std::result::Result<ClientSession, String>;

pub(crate) struct WorldStartAttempt {
    pub(crate) world_id: Uuid,
    pub(crate) receiver: Mutex<Receiver<WorldStartResult>>,
}

/// Minimum time the loading splash stays at full opacity before it is
/// allowed to fade. Even on instant local loads the splash sticks around
/// long enough to read, so players always get a clear loading beat instead
/// of a single-frame flash. Tuned by feel: short enough to not feel
/// artificial, long enough to register.
///
/// The `Startup` variant overrides this with [`LOADING_SPLASH_STARTUP_MIN_HOLD_SECONDS`]
/// so the boot overlay sticks around long enough to cover the menu
/// backdrop warmup; the world-entry variants use the default.
pub(crate) const LOADING_SPLASH_MIN_HOLD_SECONDS: f32 = 0.9;
/// Crossfade duration from full splash to fully revealed scene. Matches
/// the menu backdrop fade so the two transitions feel like one motion.
pub(crate) const LOADING_SPLASH_FADE_SECONDS: f32 = 0.5;
/// Lower bound for the startup splash hold. The actual fade trigger is
/// the menu backdrop finishing its warmup (see
/// `MenuBackdropVisibility::has_finished_warmup`), so the splash and the
/// backdrop crossfade as one motion; this constant just guarantees a
/// minimum on-screen time even if warmup were to skip.
pub(crate) const LOADING_SPLASH_STARTUP_MIN_HOLD_SECONDS: f32 = 1.2;
/// Consecutive frames the world-entry readiness condition (Welcome applied,
/// live scene geometry spawned, local player replicated) must hold before the
/// splash is allowed to fade. A few frames give the renderer time to actually
/// draw the freshly-spawned scene, so the crossfade reveals a live world
/// instead of an empty frame that pops in a moment later.
pub(crate) const WORLD_ENTRY_SETTLE_FRAMES: u32 = 3;
/// Safety valve for the world-entry gate: if a readiness signal never arrives
/// (e.g. a replication hiccup), reveal the world anyway once the splash has
/// been up this long so the player is never stranded on the loading overlay.
pub(crate) const WORLD_ENTRY_READY_TIMEOUT_SECONDS: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadingSplashKind {
    /// Initial app-launch splash. Stays visible while the menu backdrop
    /// warms up (and, in the future, while WorkOS authenticates).
    Startup,
    /// Local world load between clicking Start and the scene appearing.
    EnteringWorld,
    /// Remote connection attempt to a dedicated server.
    JoiningServer,
}

/// Generic loading overlay used at app launch and when the player commits
/// to entering or joining a world. Owns its own timer + ready flag so the
/// UI layer can compute alpha without poking session state, and so the
/// minimum-display contract is honoured even when the underlying work
/// finishes in a single frame.
#[derive(Debug, Clone)]
pub(crate) struct LoadingSplash {
    pub(crate) kind: LoadingSplashKind,
    pub(crate) target_label: String,
    pub(crate) elapsed_seconds: f32,
    pub(crate) ready: bool,
    /// Consecutive frames the world-entry readiness condition has held. Drives
    /// the settle window in [`Self::note_world_ready`]; reset to 0 whenever the
    /// condition lapses so a transient blip can't fade the splash early.
    world_ready_frames: u32,
}

impl LoadingSplash {
    pub(crate) fn new(kind: LoadingSplashKind, target_label: impl Into<String>) -> Self {
        Self {
            kind,
            target_label: target_label.into(),
            elapsed_seconds: 0.0,
            ready: false,
            world_ready_frames: 0,
        }
    }

    /// Gate a world-entry splash's fade on the world actually being ready to
    /// interact with. `world_ready` is the AND of: the `Welcome` has been
    /// applied, the live scene geometry has been spawned, and the local
    /// player's replicated entity has arrived. The condition must hold for
    /// [`WORLD_ENTRY_SETTLE_FRAMES`] consecutive frames so the renderer has
    /// drawn the scene before we reveal it.
    ///
    /// No-op for the `Startup` splash (its readiness is driven by the menu
    /// backdrop warmup in the UI layer) and once `ready` is already set.
    /// Falls back to revealing the world after
    /// [`WORLD_ENTRY_READY_TIMEOUT_SECONDS`] so a missing signal can't strand
    /// the player on the splash.
    pub(crate) fn note_world_ready(&mut self, world_ready: bool) {
        if self.ready || self.kind == LoadingSplashKind::Startup {
            return;
        }
        if world_ready {
            self.world_ready_frames = self.world_ready_frames.saturating_add(1);
            if self.world_ready_frames >= WORLD_ENTRY_SETTLE_FRAMES {
                self.ready = true;
            }
        } else if self.elapsed_seconds >= WORLD_ENTRY_READY_TIMEOUT_SECONDS {
            self.ready = true;
        } else {
            self.world_ready_frames = 0;
        }
    }

    /// Startup splash shown on app launch. Title text is set immediately;
    /// the readiness flag is flipped later by the splash UI tick once the
    /// menu backdrop finishes warming up.
    pub(crate) fn startup() -> Self {
        Self::new(LoadingSplashKind::Startup, "Verifying your account…")
    }

    pub(crate) fn title(&self) -> &'static str {
        match self.kind {
            LoadingSplashKind::Startup => "Authenticating",
            LoadingSplashKind::EnteringWorld => "Entering World",
            LoadingSplashKind::JoiningServer => "Joining Server",
        }
    }

    fn min_hold_seconds(&self) -> f32 {
        match self.kind {
            LoadingSplashKind::Startup => LOADING_SPLASH_STARTUP_MIN_HOLD_SECONDS,
            LoadingSplashKind::EnteringWorld | LoadingSplashKind::JoiningServer => {
                LOADING_SPLASH_MIN_HOLD_SECONDS
            }
        }
    }

    /// Advance the timer and return the splash overlay alpha for this
    /// frame. Returns `None` once the splash has fully faded so the caller
    /// can drop it.
    pub(crate) fn tick(&mut self, delta_seconds: f32) -> Option<u8> {
        self.elapsed_seconds = (self.elapsed_seconds + delta_seconds.max(0.0)).min(60.0);
        let hold = self.min_hold_seconds();
        let fade = LOADING_SPLASH_FADE_SECONDS;
        if !self.ready || self.elapsed_seconds < hold {
            return Some(u8::MAX);
        }
        let into_fade = self.elapsed_seconds - hold;
        if into_fade >= fade {
            return None;
        }
        let alpha = ((1.0 - into_fade / fade).clamp(0.0, 1.0) * f32::from(u8::MAX)).round() as u8;
        Some(alpha)
    }
}

pub(crate) struct DirectConnectDialog {
    pub(crate) host: String,
    pub(crate) port: String,
    pub(crate) error: Option<String>,
    pub(crate) attempt: Option<DirectConnectAttempt>,
}

impl DirectConnectDialog {
    pub(crate) fn new(address: &str) -> Self {
        let (host, port) = split_host_port(address);
        Self {
            host,
            port,
            error: None,
            attempt: None,
        }
    }

    pub(crate) fn is_connecting(&self) -> bool {
        self.attempt.is_some()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CreateWorldDialog {
    pub(crate) name: String,
    pub(crate) procedural_size: ProceduralMapSize,
    pub(crate) seed: String,
    pub(crate) error: Option<String>,
    pub(crate) closing: bool,
    pub(crate) confirmed: bool,
    /// Set on open, cleared after the name field grabs focus on its first
    /// frame, so the player can name-and-Enter without reaching for the mouse.
    pub(crate) autofocus_pending: bool,
}

impl Default for CreateWorldDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl CreateWorldDialog {
    pub(crate) fn new() -> Self {
        Self {
            name: "New World".to_owned(),
            procedural_size: ProceduralMapSize::Medium,
            seed: random_seed().to_string(),
            error: None,
            closing: false,
            confirmed: false,
            autofocus_pending: true,
        }
    }

    pub(crate) fn refresh_seed(&mut self) {
        self.seed = random_seed().to_string();
        self.error = None;
    }

    pub(crate) fn selected_map(&self) -> Result<MapType, &'static str> {
        let seed = self
            .seed
            .trim()
            .parse::<u64>()
            .map_err(|_| "Seed must be a whole number.")?;
        Ok(MapType::Procedural {
            seed,
            size: self.procedural_size,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EditWorldDialog {
    pub(crate) world_id: Uuid,
    pub(crate) name: String,
    pub(crate) map: MapType,
    pub(crate) error: Option<String>,
    pub(crate) closing: bool,
    pub(crate) confirmed: bool,
    /// Set on open, cleared after the name field grabs focus on its first
    /// frame (which also selects the existing name for quick replacement).
    pub(crate) autofocus_pending: bool,
}

impl EditWorldDialog {
    pub(crate) fn new(world: &WorldSummary) -> Self {
        Self {
            world_id: world.id,
            name: world.name.clone(),
            map: world.map.clone(),
            error: None,
            closing: false,
            confirmed: false,
            autofocus_pending: true,
        }
    }
}

fn random_seed() -> u64 {
    let bytes = Uuid::new_v4().into_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

fn split_host_port(address: &str) -> (String, String) {
    match address.parse::<std::net::SocketAddr>() {
        Ok(addr) => (addr.ip().to_string(), addr.port().to_string()),
        Err(_) => address
            .rsplit_once(':')
            .map(|(host, port)| {
                (
                    host.trim_matches(['[', ']']).trim().to_owned(),
                    port.trim().to_owned(),
                )
            })
            .unwrap_or_else(|| (address.trim().to_owned(), "7777".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_entry_splash_holds_until_readiness_settles() {
        let mut splash = LoadingSplash::new(LoadingSplashKind::JoiningServer, "host");
        // World not ready: the fade is never armed no matter how many frames.
        for _ in 0..10 {
            splash.note_world_ready(false);
            assert!(!splash.ready);
        }
        // Readiness must hold for the full settle window before arming.
        for _ in 0..WORLD_ENTRY_SETTLE_FRAMES - 1 {
            splash.note_world_ready(true);
            assert!(!splash.ready);
        }
        splash.note_world_ready(true);
        assert!(
            splash.ready,
            "splash should arm the fade after the settle window"
        );
    }

    #[test]
    fn world_entry_settle_resets_when_readiness_lapses() {
        let mut splash = LoadingSplash::new(LoadingSplashKind::EnteringWorld, "World");
        // A couple of ready frames, then a lapse: the counter resets so a
        // transient blip can't fade the splash early.
        splash.note_world_ready(true);
        splash.note_world_ready(true);
        splash.note_world_ready(false);
        assert!(!splash.ready);
        // A fresh, uninterrupted run of ready frames is then required.
        for _ in 0..WORLD_ENTRY_SETTLE_FRAMES {
            assert!(!splash.ready);
            splash.note_world_ready(true);
        }
        assert!(splash.ready);
    }

    #[test]
    fn world_entry_timeout_reveals_world_without_a_ready_signal() {
        let mut splash = LoadingSplash::new(LoadingSplashKind::JoiningServer, "host");
        // Past the safety-valve timeout the world is revealed even with no
        // readiness signal, so the player is never stranded on the splash.
        splash.elapsed_seconds = WORLD_ENTRY_READY_TIMEOUT_SECONDS;
        splash.note_world_ready(false);
        assert!(splash.ready, "the timeout fallback must reveal the world");
    }

    #[test]
    fn startup_splash_ignores_world_readiness() {
        let mut splash = LoadingSplash::startup();
        for _ in 0..WORLD_ENTRY_SETTLE_FRAMES + 2 {
            splash.note_world_ready(true);
        }
        assert!(
            !splash.ready,
            "Startup readiness is driven by backdrop warmup, not world readiness"
        );
    }

    #[test]
    fn auth_rejected_notice_surfaces_reason_when_present() {
        let notice = NoticeDialog::auth_rejected("protocol mismatch: client 3, server 4");
        assert_eq!(notice.title, "Couldn't join server");
        assert!(
            notice
                .body
                .contains("protocol mismatch: client 3, server 4")
        );
        assert_eq!(notice.confirm_label, "OK");
    }

    #[test]
    fn auth_rejected_notice_falls_back_when_reason_is_blank() {
        let notice = NoticeDialog::auth_rejected("   ");
        assert_eq!(notice.title, "Couldn't join server");
        assert!(
            notice.body.contains("login couldn't be verified"),
            "a blank reason should still read cleanly: {}",
            notice.body
        );
    }

    #[test]
    fn version_mismatch_notice_shows_both_versions_and_direction() {
        let older = NoticeDialog::version_mismatch("0.14.0", "0.15.0");
        assert_eq!(older.title, "Version mismatch");
        assert!(older.body.contains("0.14.0"), "shows the client version");
        assert!(older.body.contains("0.15.0"), "shows the server version");
        assert!(older.body.contains("older"), "flags the client as older");

        let newer = NoticeDialog::version_mismatch("0.16.0", "0.15.0");
        assert!(newer.body.contains("newer"), "flags the client as newer");

        // Unparseable versions fall back to neutral phrasing instead of
        // guessing a direction, but still show both versions.
        let weird = NoticeDialog::version_mismatch("dev", "0.15.0");
        assert!(weird.body.contains("0.15.0"));
        assert!(!weird.body.contains("older") && !weird.body.contains("newer"));
    }

    #[test]
    fn compare_versions_is_numeric_and_tolerates_suffixes() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("0.14.1", "0.14.2"), Some(Ordering::Less));
        // Numeric, not lexical: 10 > 2 even though "10" < "2" as strings.
        assert_eq!(
            compare_versions("0.14.10", "0.14.2"),
            Some(Ordering::Greater)
        );
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Some(Ordering::Equal));
        // A pre-release suffix on the patch is stripped to its leading digits.
        assert_eq!(
            compare_versions("0.14.1-beta", "0.14.1"),
            Some(Ordering::Equal)
        );
        assert_eq!(compare_versions("oops", "0.1.0"), None);
    }
}
