use bevy::prelude::*;

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
