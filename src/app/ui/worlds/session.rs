use std::{
    sync::{
        Mutex,
        mpsc::{self, TryRecvError},
    },
    thread,
};

use anyhow::Context;
use uuid::Uuid;

use crate::{
    app::state::{
        ClientRuntime, MenuState, SaveStore, Screen, SteamUser, WorldEntryKind, WorldEntrySplash,
        WorldStartAttempt, WorldStartResult,
    },
    net::ClientSession,
    save::WorldStore,
    steam::AuthenticatedUser,
};

pub(in crate::app::ui) fn refresh_worlds(menu: &mut MenuState, store: &SaveStore) {
    match store.0.list_worlds() {
        Ok(listing) => {
            menu.worlds = listing.worlds;
            menu.corrupted_worlds = listing.corrupted;
            menu.status = None;
        }
        Err(error) => {
            menu.worlds.clear();
            menu.corrupted_worlds.clear();
            menu.status = Some(format!("world list failed: {error}"));
        }
    }
}

#[cfg(test)]
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

pub(super) fn poll_singleplayer_start(menu: &mut MenuState, runtime: &mut ClientRuntime) -> bool {
    let Some((world_id, result)) = take_finished_singleplayer_start(menu) else {
        return menu.world_start.is_some();
    };

    finish_singleplayer_start(menu, runtime, world_id, result);
    false
}

pub(super) fn start_singleplayer_in_background(
    menu: &mut MenuState,
    store: &SaveStore,
    user: &SteamUser,
    world_id: Uuid,
) {
    if menu.world_start.is_some() {
        return;
    }

    let (tx, receiver) = mpsc::channel::<WorldStartResult>();
    let store = store.0.clone();
    let user = user.0.clone();
    match thread::Builder::new()
        .name("singleplayer-start".to_owned())
        .spawn(move || {
            let result = start_singleplayer_session(store, user, world_id)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        }) {
        Ok(_) => {
            let world_name = menu
                .worlds
                .iter()
                .find(|world| world.id == world_id)
                .map(|world| world.name.clone())
                .unwrap_or_default();
            menu.world_start = Some(WorldStartAttempt {
                world_id,
                receiver: Mutex::new(receiver),
            });
            menu.world_entry_splash =
                Some(WorldEntrySplash::new(WorldEntryKind::Singleplayer, world_name));
            menu.status = None;
        }
        Err(error) => {
            menu.status = Some(format!("start failed: could not start loader: {error}"));
        }
    }
}

fn start_singleplayer_session(
    store: WorldStore,
    user: AuthenticatedUser,
    world_id: Uuid,
) -> anyhow::Result<ClientSession> {
    store
        .load_world(world_id)
        .context("could not load selected world")
        .and_then(|save| ClientSession::start_singleplayer(save, &user))
}

fn take_finished_singleplayer_start(menu: &mut MenuState) -> Option<(Uuid, WorldStartResult)> {
    enum StartPoll {
        Result(std::result::Result<WorldStartResult, TryRecvError>),
        Poisoned,
    }

    let attempt = menu.world_start.as_ref()?;
    let world_id = attempt.world_id;
    let poll = match attempt.receiver.lock() {
        Ok(receiver) => StartPoll::Result(receiver.try_recv()),
        Err(_) => StartPoll::Poisoned,
    };

    match poll {
        StartPoll::Poisoned => {
            menu.world_start = None;
            Some((world_id, Err("start state is unavailable".to_owned())))
        }
        StartPoll::Result(Ok(result)) => {
            menu.world_start = None;
            Some((world_id, result))
        }
        StartPoll::Result(Err(TryRecvError::Empty)) => None,
        StartPoll::Result(Err(TryRecvError::Disconnected)) => {
            menu.world_start = None;
            Some((
                world_id,
                Err("start attempt ended before returning a result".to_owned()),
            ))
        }
    }
}

fn finish_singleplayer_start(
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    world_id: Uuid,
    result: WorldStartResult,
) {
    match result {
        Ok(session) => {
            runtime.start_session(session, Some(world_id));
            menu.screen = Screen::InGame;
            menu.pause_open = false;
            menu.pause_options_open = false;
            menu.chat_open = false;
            menu.chat_focus_pending = false;
            menu.status = None;
            if let Some(splash) = menu.world_entry_splash.as_mut() {
                splash.world_ready = true;
            }
        }
        Err(error) => {
            menu.status = Some(format!("start failed: {error}"));
            menu.world_entry_splash = None;
        }
    }
}
