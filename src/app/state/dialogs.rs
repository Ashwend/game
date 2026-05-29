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
}

#[derive(Debug, Clone)]
pub(crate) enum ConfirmationAction {
    DeleteWorld { world_id: Uuid },
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
}

pub(crate) type DirectConnectResult = std::result::Result<(SocketAddr, ClientSession), String>;

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
    /// warms up (and, in the future, while Steam authenticates).
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
}
