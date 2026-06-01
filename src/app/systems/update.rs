//! Bridges the update checker ([`crate::update`]) to the world-save-on-quit
//! path. Lives in `app` (not `crate::update`) because applying an update has to
//! coordinate with `ClientRuntime`/`MenuState`/`SessionShutdownTasks`, which are
//! private to the `app` module.
//!
//! When the player chooses **Restart & update**, [`UpdateState`] flips to
//! [`UpdateStatus::Applying`]. This system then guarantees any open
//! singleplayer world is saved before the process exits: a live session is torn
//! down via the same background save the pause-menu Quit uses, then it waits for
//! that save to finish, and only then launches the updater and quits. From a
//! menu screen (no session) it applies immediately.

use bevy::{app::AppExit, prelude::*};

use crate::{
    analytics::SessionEndReason,
    app::state::{ClientRuntime, MenuState, SaveStore, Screen, SessionShutdownTasks},
    update::{UpdateState, UpdateStatus},
};

use super::PendingSessionEndReason;

pub(crate) fn apply_update_system(
    mut update: ResMut<UpdateState>,
    mut runtime: ResMut<ClientRuntime>,
    store: Res<SaveStore>,
    mut shutdown_tasks: ResMut<SessionShutdownTasks>,
    mut menu: ResMut<MenuState>,
    mut pending_session_end: ResMut<PendingSessionEndReason>,
    mut app_exit: MessageWriter<AppExit>,
) {
    if !matches!(update.status, UpdateStatus::Applying) {
        return;
    }

    // A live session must be torn down (and its world saved) before we replace
    // the binary and relaunch. This branch runs once: `shutdown_in_background`
    // clears the session, so subsequent frames fall through to the wait below.
    if runtime.session.is_some() {
        pending_session_end.0 = Some(SessionEndReason::UserQuit);
        runtime.shutdown_in_background(store.0.clone(), &mut shutdown_tasks);
        menu.screen = Screen::MainMenu;
        menu.pause_open = false;
        menu.pause_options_open = false;
        menu.inventory_open = false;
        menu.chat_open = false;
        menu.chat_focus_pending = false;
        return;
    }

    // Hold the relaunch until any in-flight world save is durable.
    if !shutdown_tasks.all_finished() {
        return;
    }

    let Some(staged) = update.staged_path() else {
        update.fail("no staged update to apply");
        return;
    };

    match crate::update::spawn_updater(&staged) {
        Ok(()) => {
            app_exit.write(AppExit::Success);
        }
        Err(error) => update.fail(format!("could not start updater: {error}")),
    }
}
