use bevy::prelude::*;

use super::super::state::{ClientSettings, ClientSettingsStore};

const SETTINGS_SAVE_DEBOUNCE_SECONDS: f32 = 0.35;

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
