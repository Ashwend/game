use std::{
    net::SocketAddr,
    sync::{
        Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};

use bevy::prelude::*;

use crate::{
    app::state::{ClientRuntime, LoadingSplash, LoadingSplashKind, MenuState, Screen, SteamUser},
    net::{ClientNetwork, ClientSession},
    steam::AuthenticatedUser,
};

/// Optional one-shot directive that asks the client to skip the main menu
/// and connect to a specific server address as soon as it boots. Populated
/// by the `multiplayer-test` CLI command so spawned client windows enter the
/// shared test world without any clicking required.
#[derive(Resource, Debug, Clone)]
pub(crate) struct AutoConnectRequest {
    pub(crate) addr: SocketAddr,
}

/// Live state of an auto-connect attempt. Holds the worker thread's result
/// channel; consumed once the attempt completes.
#[derive(Resource)]
pub(crate) struct AutoConnectAttempt {
    receiver: Mutex<Receiver<AutoConnectResult>>,
}

type AutoConnectResult = Result<(SocketAddr, ClientSession), String>;

/// Boot-time system: when an [`AutoConnectRequest`] is present and no
/// attempt has been kicked off yet, spawn the worker thread and flip the
/// startup splash to a "Joining Server" overlay so the player sees what's
/// happening instead of the default authenticating-into-menu splash.
pub(crate) fn auto_connect_start_system(
    mut commands: Commands,
    request: Option<Res<AutoConnectRequest>>,
    attempt: Option<Res<AutoConnectAttempt>>,
    user: Res<SteamUser>,
    network: Res<ClientNetwork>,
    mut menu: ResMut<MenuState>,
) {
    if attempt.is_some() {
        return;
    }
    let Some(request) = request else {
        return;
    };

    let (tx, rx) = mpsc::channel::<AutoConnectResult>();
    let user_clone = user.0.clone();
    let network_clone = network.clone();
    let target = request.addr;
    if let Err(error) = thread::Builder::new()
        .name("auto-connect-attempt".to_owned())
        .spawn(move || {
            let result =
                connect(target, user_clone, network_clone).map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        })
    {
        eprintln!("could not spawn auto-connect attempt: {error:#}");
        commands.remove_resource::<AutoConnectRequest>();
        return;
    }

    menu.loading_splash = Some(LoadingSplash::new(
        LoadingSplashKind::JoiningServer,
        target.to_string(),
    ));
    commands.insert_resource(AutoConnectAttempt {
        receiver: Mutex::new(rx),
    });
}

fn connect(
    addr: SocketAddr,
    user: AuthenticatedUser,
    network: ClientNetwork,
) -> anyhow::Result<(SocketAddr, ClientSession)> {
    let session = ClientSession::connect(addr, &user, network)?;
    Ok((addr, session))
}

/// Polls the worker thread; once the attempt finishes, hands the session to
/// the runtime, drops the request/attempt resources, and switches the
/// foreground screen to `InGame`. On failure the request is also cleared and
/// the error surfaces via `menu.status` so the player can re-launch the
/// helper.
pub(crate) fn auto_connect_poll_system(
    mut commands: Commands,
    attempt: Option<Res<AutoConnectAttempt>>,
    mut menu: ResMut<MenuState>,
    mut runtime: ResMut<ClientRuntime>,
) {
    let Some(attempt) = attempt else {
        return;
    };

    let polled = {
        let Ok(receiver) = attempt.receiver.lock() else {
            return;
        };
        receiver.try_recv()
    };

    match polled {
        Ok(Ok((addr, session))) => {
            runtime.start_session(session, None);
            menu.multiplayer_addr = addr.to_string();
            menu.screen = Screen::InGame;
            menu.pause_open = false;
            menu.pause_options_open = false;
            menu.chat_open = false;
            menu.chat_focus_pending = false;
            menu.status = None;
            if let Some(splash) = menu.loading_splash.as_mut() {
                splash.ready = true;
            }
            commands.remove_resource::<AutoConnectAttempt>();
            commands.remove_resource::<AutoConnectRequest>();
        }
        Ok(Err(error)) => {
            eprintln!("auto-connect failed: {error}");
            menu.status = Some(format!("Auto-connect failed: {error}"));
            menu.loading_splash = None;
            commands.remove_resource::<AutoConnectAttempt>();
            commands.remove_resource::<AutoConnectRequest>();
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            menu.status = Some("Auto-connect helper exited before returning a result.".to_owned());
            menu.loading_splash = None;
            commands.remove_resource::<AutoConnectAttempt>();
            commands.remove_resource::<AutoConnectRequest>();
        }
    }
}
