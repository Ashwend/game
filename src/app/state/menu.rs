use bevy::prelude::*;

use crate::{save::WorldStore, steam::AuthenticatedUser};

use super::{
    ConfirmationDialog, CreateWorldDialog, DirectConnectDialog, EditWorldDialog, WorldStartAttempt,
};

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
    pub(crate) create_world: Option<CreateWorldDialog>,
    pub(crate) edit_world: Option<EditWorldDialog>,
    pub(crate) direct_connect: Option<DirectConnectDialog>,
    pub(crate) world_start: Option<WorldStartAttempt>,
    pub(crate) multiplayer_addr: String,
    pub(crate) status: Option<String>,
    pub(crate) pause_open: bool,
    pub(crate) pause_options_open: bool,
    pub(crate) inventory_open: bool,
    pub(crate) chat_open: bool,
    pub(crate) chat_focus_pending: bool,
    pub(crate) chat_input: String,
    pub(crate) confirmation: Option<ConfirmationDialog>,
    pub(crate) quit_requested: bool,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            screen: Screen::MainMenu,
            worlds: Vec::new(),
            create_world: None,
            edit_world: None,
            direct_connect: None,
            world_start: None,
            multiplayer_addr: "127.0.0.1:7777".to_owned(),
            status: None,
            pause_open: false,
            pause_options_open: false,
            inventory_open: false,
            chat_open: false,
            chat_focus_pending: false,
            chat_input: String::new(),
            confirmation: None,
            quit_requested: false,
        }
    }
}
