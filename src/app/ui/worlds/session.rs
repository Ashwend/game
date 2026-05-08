use anyhow::Context;
use uuid::Uuid;

use crate::{
    app::state::{ClientRuntime, MenuState, SaveStore, Screen, SteamUser},
    net::ClientSession,
};

pub(in crate::app::ui) fn refresh_worlds(menu: &mut MenuState, store: &SaveStore) {
    match store.0.list_worlds() {
        Ok(worlds) => {
            menu.worlds = worlds;
            menu.status = None;
        }
        Err(error) => {
            menu.worlds.clear();
            menu.status = Some(format!("world list failed: {error}"));
        }
    }
}

pub(super) fn start_singleplayer(
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
    world_id: Uuid,
) {
    let result = store
        .0
        .load_world(world_id)
        .context("could not load selected world")
        .and_then(|save| ClientSession::start_singleplayer(save, &user.0));

    match result {
        Ok(session) => {
            runtime.start_session(session, Some(world_id));
            menu.screen = Screen::InGame;
            menu.pause_open = false;
            menu.pause_options_open = false;
            menu.chat_open = false;
            menu.chat_focus_pending = false;
            menu.status = None;
        }
        Err(error) => menu.status = Some(format!("start failed: {error}")),
    }
}
