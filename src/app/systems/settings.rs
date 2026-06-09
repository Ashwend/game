use bevy::{app::AppExit, prelude::*};

use super::super::state::{ClientRuntime, ClientSettings, ClientSettingsStore};
use crate::protocol::{ClientMessage, ViewRadiusTier};

const SETTINGS_SAVE_DEBOUNCE_SECONDS: f32 = 0.35;

/// Mirror the user's view-radius setting to the server. Fires whenever the
/// setting changes *and* a session is live; resets on disconnect so the
/// next reconnect re-sends the tier (otherwise the server would never
/// learn the player's preference after a Welcome).
pub(crate) fn sync_view_radius_system(
    settings: Res<ClientSettings>,
    mut runtime: ResMut<ClientRuntime>,
    mut last_sent: Local<Option<ViewRadiusTier>>,
) {
    let desired = settings.hud.view_radius;
    if runtime.session.is_none() {
        *last_sent = None;
        return;
    }
    if *last_sent == Some(desired) {
        return;
    }
    if let Some(session) = runtime.session.as_mut()
        && session
            .send(ClientMessage::SetViewRadius { tier: desired })
            .is_ok()
    {
        *last_sent = Some(desired);
    }
}

pub(crate) fn save_client_settings_system(
    time: Res<Time>,
    settings: Res<ClientSettings>,
    store: Res<ClientSettingsStore>,
    mut pending_save: Local<Option<Timer>>,
) {
    if settings.is_changed() {
        let timer = pending_save.get_or_insert_with(|| {
            Timer::from_seconds(SETTINGS_SAVE_DEBOUNCE_SECONDS, TimerMode::Once)
        });
        timer.reset();
    }

    let Some(timer) = pending_save.as_mut() else {
        return;
    };
    timer.tick(time.delta());
    if !timer.is_finished() {
        return;
    }

    if let Err(error) = store.save(&settings) {
        warn!("could not save client settings: {error}");
    }
    *pending_save = None;
}

/// Persist settings on quit, regardless of the debounce above.
///
/// The options panel marks `ClientSettings` changed every frame it's open (the
/// egui code takes `&mut`), so [`save_client_settings_system`]'s debounce keeps
/// resetting and never fires *while* you're in the menu, only ~0.35s after you
/// leave it. Quitting straight from the settings screen (or within that window)
/// would otherwise drop your most recent change, which is exactly the "I turned
/// X off, restarted, and it was back on" report.
///
/// Runs in `Last`: an `Update` reader never observes the window-close `AppExit`
/// (the app stops the same frame Bevy writes it, in `PostUpdate`), the same
/// reason the analytics drain runs in `Last`. Saving the whole settings blob is
/// a single small file write, so doing it unconditionally on exit is fine.
pub(crate) fn flush_settings_on_exit_system(
    settings: Res<ClientSettings>,
    store: Res<ClientSettingsStore>,
    mut exit: MessageReader<AppExit>,
) {
    if exit.read().next().is_none() {
        return;
    }
    if let Err(error) = store.save(&settings) {
        warn!("could not save client settings on exit: {error}");
    }
}
