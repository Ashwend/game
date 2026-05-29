use bevy::prelude::*;

use crate::{
    save::{CorruptedWorld, WorldStore},
    steam::AuthenticatedUser,
};

use super::{
    ConfirmationDialog, CreateWorldDialog, DirectConnectDialog, EditWorldDialog, LoadingSplash,
    NoticeDialog, WorldStartAttempt,
};

pub(crate) const DEFAULT_MULTIPLAYER_ADDR: &str = "46.224.101.205:7777";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    MainMenu,
    Options,
    Worlds,
    Multiplayer,
    InGame,
}

impl Screen {
    pub(crate) fn uses_menu_backdrop(self) -> bool {
        self != Self::InGame
    }
}

#[derive(Resource)]
pub(crate) struct SaveStore(pub(crate) WorldStore);

#[derive(Resource)]
pub(crate) struct SteamUser(pub(crate) AuthenticatedUser);

#[derive(Resource)]
pub(crate) struct MenuState {
    pub(crate) screen: Screen,
    pub(crate) worlds: Vec<crate::save::WorldSummary>,
    /// Saves that were present on disk but could not be parsed (truncated,
    /// bad header, mismatched format version, …). Rendered above the worlds
    /// table as a separate "couldn't load" group so the player knows there
    /// are files that need attention rather than seeing them silently
    /// dropped from the list.
    pub(crate) corrupted_worlds: Vec<CorruptedWorld>,
    pub(crate) create_world: Option<CreateWorldDialog>,
    pub(crate) edit_world: Option<EditWorldDialog>,
    pub(crate) direct_connect: Option<DirectConnectDialog>,
    pub(crate) world_start: Option<WorldStartAttempt>,
    /// Loading overlay shown on top of every screen. Set on app launch
    /// (with the `Startup` kind) and again whenever the player commits
    /// to entering a world. Lives in `MenuState` (not `ClientRuntime`)
    /// because it's a UI-only artifact: gameplay state advances normally
    /// underneath.
    pub(crate) loading_splash: Option<LoadingSplash>,
    pub(crate) multiplayer_addr: String,
    pub(crate) status: Option<String>,
    pub(crate) pause_open: bool,
    pub(crate) pause_options_open: bool,
    pub(crate) inventory_open: bool,
    /// Whether the dedicated crafting screen is up. Like
    /// `inventory_open`, this only frees the cursor and gates input — it
    /// does not pause gameplay or the network tick. The server keeps
    /// progressing the queue regardless of this flag.
    pub(crate) crafting_open: bool,
    /// Mirrors `local_player.open_furnace.is_some()` from the snapshot.
    /// Used by the input gating helpers to suppress movement/look while
    /// the furnace modal is up, without those helpers having to reach
    /// into the snapshot themselves. Synced by
    /// [`crate::app::systems::sync_furnace_open_flag_system`].
    pub(crate) furnace_open: bool,
    /// Mirrors `local_player.open_loot_bag.is_some()` from
    /// replication. Same purpose as `furnace_open` — input gating
    /// without reaching into the replicated state from every
    /// helper.
    pub(crate) loot_bag_open: bool,
    pub(crate) chat_open: bool,
    pub(crate) chat_focus_pending: bool,
    pub(crate) chat_input: String,
    pub(crate) confirmation: Option<ConfirmationDialog>,
    pub(crate) notice: Option<NoticeDialog>,
    /// Set when the server tells the local client they died. Drives
    /// the "You died — Killed by … — Respawn" splash. Cleared when the
    /// respawn lands (server pushes a `Correction` and the runtime
    /// flips back to alive) so the splash auto-dismisses.
    pub(crate) death_splash: Option<DeathSplash>,
    pub(crate) quit_requested: bool,
}

/// Snapshot of "I just died" UI state. Stored on `MenuState` because
/// the splash sits in the same UI layer as the pause/inventory
/// overlays and shares their input gating.
#[derive(Debug, Clone)]
pub(crate) struct DeathSplash {
    /// Display name of the killer, resolved server-side. `None` for
    /// environmental death (future) where there's no attacker to
    /// credit.
    pub(crate) killer_name: Option<String>,
    /// Seconds since the splash opened. Drives the slow fade from
    /// "the player still sees the world dying" into the fully-black
    /// "YOU DIED" screen.
    pub(crate) elapsed: f32,
    /// Seconds spent in the closing-fade animation. `None` until the
    /// respawn `Correction` lands — at that point the splash keeps
    /// rendering but its backdrop alpha eases from "fully black" back
    /// to "fully transparent" over [`CLOSE_FADE_SECS`]. The splash
    /// clears itself when the fade completes, so the new HUD doesn't
    /// pop in for a frame underneath a still-black screen.
    pub(crate) closing_elapsed: Option<f32>,
}

impl DeathSplash {
    pub(crate) fn new(killer_name: Option<String>) -> Self {
        Self {
            killer_name,
            elapsed: 0.0,
            closing_elapsed: None,
        }
    }

    /// Start the close-fade. Idempotent — once started, the same
    /// timer keeps ticking; a second call is a no-op so racing
    /// `Correction` messages can't reset the curve and leave the
    /// player staring at black.
    pub(crate) fn begin_closing(&mut self) {
        if self.closing_elapsed.is_none() {
            self.closing_elapsed = Some(0.0);
        }
    }
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            screen: Screen::MainMenu,
            worlds: Vec::new(),
            corrupted_worlds: Vec::new(),
            create_world: None,
            edit_world: None,
            direct_connect: None,
            world_start: None,
            loading_splash: Some(LoadingSplash::startup()),
            multiplayer_addr: DEFAULT_MULTIPLAYER_ADDR.to_owned(),
            status: None,
            pause_open: false,
            pause_options_open: false,
            inventory_open: false,
            crafting_open: false,
            furnace_open: false,
            loot_bag_open: false,
            chat_open: false,
            chat_focus_pending: false,
            chat_input: String::new(),
            confirmation: None,
            notice: None,
            death_splash: None,
            quit_requested: false,
        }
    }
}

impl MenuState {
    /// Hand off to the in-game screen after a successful singleplayer or
    /// multiplayer session start. Resets the modal/chat overlay state. Both
    /// session-start paths (loopback singleplayer and direct multiplayer)
    /// funnel through here so the two flows can't drift in what they clear.
    ///
    /// This flips the *screen* (so the scene starts building underneath the
    /// splash) but deliberately does NOT mark the loading splash ready to
    /// fade — that's gated on the world actually being ready to interact with
    /// (Welcome applied, scene spawned, local player replicated) via
    /// `LoadingSplash::note_world_ready`, driven from the UI each frame. The
    /// session object existing only means the handshake started, not that the
    /// world has arrived or rendered.
    pub(crate) fn enter_in_game(&mut self) {
        self.screen = Screen::InGame;
        self.pause_open = false;
        self.pause_options_open = false;
        self.crafting_open = false;
        self.furnace_open = false;
        self.loot_bag_open = false;
        self.chat_open = false;
        self.chat_focus_pending = false;
        self.status = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn death_splash_new_has_no_killer_and_no_closing_timer() {
        let splash = DeathSplash::new(None);
        assert!(splash.killer_name.is_none());
        assert_eq!(splash.elapsed, 0.0);
        assert!(
            splash.closing_elapsed.is_none(),
            "the closing fade only starts after the respawn lands"
        );
    }

    #[test]
    fn death_splash_remembers_killer_name() {
        let splash = DeathSplash::new(Some("Murderer".into()));
        assert_eq!(splash.killer_name.as_deref(), Some("Murderer"));
    }

    #[test]
    fn begin_closing_starts_the_fade() {
        let mut splash = DeathSplash::new(None);
        splash.begin_closing();
        assert_eq!(
            splash.closing_elapsed,
            Some(0.0),
            "first call should start the closing timer at 0"
        );
    }

    #[test]
    fn begin_closing_is_idempotent() {
        // A second `Correction` that lands while the fade is already
        // running must not reset the curve — that would freeze the
        // player on a black screen for an extra fade window.
        let mut splash = DeathSplash::new(None);
        splash.begin_closing();
        splash.closing_elapsed = Some(0.25);
        splash.begin_closing();
        assert_eq!(splash.closing_elapsed, Some(0.25));
    }
}
