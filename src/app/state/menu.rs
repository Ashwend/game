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
    pub(crate) chat_open: bool,
    pub(crate) chat_focus_pending: bool,
    pub(crate) chat_input: String,
    pub(crate) confirmation: Option<ConfirmationDialog>,
    pub(crate) notice: Option<NoticeDialog>,
    pub(crate) quit_requested: bool,
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
            chat_open: false,
            chat_focus_pending: false,
            chat_input: String::new(),
            confirmation: None,
            notice: None,
            quit_requested: false,
        }
    }
}

impl MenuState {
    /// Hand off to the in-game screen after a successful singleplayer or
    /// multiplayer session start. Resets the modal/chat overlay state and
    /// flags the loading splash as ready to fade out. Both session-start
    /// paths (loopback singleplayer and direct multiplayer) funnel through
    /// here so the two flows can't drift in what they clear or set.
    pub(crate) fn enter_in_game(&mut self) {
        self.screen = Screen::InGame;
        self.pause_open = false;
        self.pause_options_open = false;
        self.crafting_open = false;
        self.furnace_open = false;
        self.chat_open = false;
        self.chat_focus_pending = false;
        self.status = None;
        if let Some(splash) = self.loading_splash.as_mut() {
            splash.ready = true;
        }
    }
}
