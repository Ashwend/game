//! Bridge between Bevy state and the [`crate::analytics`] worker.
//!
//! Holds the small observer systems that turn ambient state changes into
//! discrete analytics events:
//!
//! - [`screen_viewed_system`], fires `screen_viewed` when the visible screen
//!   changes: the login splash (`sign_in`) while logged out, otherwise the
//!   [`MenuState::screen`] the user is on.
//! - [`session_started_system`], fires `session_started` when
//!   [`ClientRuntime::session`] flips from `None` to `Some`. Mode is derived
//!   from `active_world_id` (present → singleplayer, absent → multiplayer).
//! - [`session_ended_system`], fires `session_ended` when the session goes
//!   away, with a reason taken from [`PendingSessionEndReason`].
//! - [`error_relay_system`], relays queued [`ClientErrorToast`] messages
//!   into typed `error` analytics events.
//!
//! These systems live here (not in `analytics::`) so the analytics crate
//! stays a leaf with no Bevy/app dependencies. Wire them into the schedule
//! in `app::run_app`.

use std::time::Instant;

use bevy::prelude::*;

use crate::{
    analytics::{Analytics, ErrorCategory, Event, ScreenKind, SessionEndReason, SessionMode},
    app::state::{AuthFlow, ClientErrorToast, ClientRuntime, MenuState, Screen},
};

/// Last [`ScreenKind`] we emitted `screen_viewed` for. Initialised to `None` so
/// the very first frame fires for the launch screen (the login splash for a
/// logged-out user, the main menu for a returning one).
#[derive(Resource, Default)]
pub(crate) struct LastTrackedScreen(pub(crate) Option<ScreenKind>);

/// Per-session bookkeeping: when it started + the reason set by the caller
/// (kick path, user quit). The reason is consumed once and reset to
/// [`SessionEndReason::Disconnect`] (the catch-all) on the next session.
#[derive(Resource, Default)]
pub(crate) struct SessionTracker {
    started_at: Option<Instant>,
    /// Whether `runtime.session` was `Some` on the previous frame.
    was_active: bool,
}

/// One-shot "this session is ending because of X" hint, consumed by
/// [`session_ended_system`]. Set by code that knows *why* the session is
/// going away (kick handler, pause-menu Quit button). If left untouched, we
/// assume a disconnect.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub(crate) struct PendingSessionEndReason(pub(crate) Option<SessionEndReason>);

pub(crate) fn screen_viewed_system(
    analytics: Res<Analytics>,
    auth: Res<AuthFlow>,
    menu: Res<MenuState>,
    mut last: ResMut<LastTrackedScreen>,
) {
    // The login splash is gated by `AuthFlow`, not `MenuState::screen` (which
    // sits at `MainMenu` underneath it), so derive the screen from auth first:
    // a logged-out user is looking at the sign-in screen, not the menu. The
    // `Verifying`/`Authenticating` spinners are transient sub-states of the
    // sign-in flow, emit nothing for them so a returning user's silent refresh
    // doesn't log a spurious `sign_in` view before the menu loads.
    let screen = match &*auth {
        AuthFlow::Authenticated => map_screen(menu.screen),
        AuthFlow::LoggedOut { .. } => ScreenKind::SignIn,
        AuthFlow::Verifying(_) | AuthFlow::Authenticating(_) => return,
    };
    if last.0 == Some(screen) {
        return;
    }
    last.0 = Some(screen);
    analytics.track(Event::ScreenViewed { screen });
}

pub(crate) fn session_started_system(
    analytics: Res<Analytics>,
    runtime: Res<ClientRuntime>,
    mut tracker: ResMut<SessionTracker>,
) {
    let now_active = runtime.session.is_some();
    if now_active && !tracker.was_active {
        tracker.started_at = Some(Instant::now());
        let mode = if runtime.active_world_id.is_some() {
            SessionMode::Singleplayer
        } else {
            SessionMode::Multiplayer
        };
        analytics.track(Event::SessionStarted { mode });
    }
    tracker.was_active = now_active;
}

pub(crate) fn session_ended_system(
    analytics: Res<Analytics>,
    runtime: Res<ClientRuntime>,
    mut tracker: ResMut<SessionTracker>,
    mut reason: ResMut<PendingSessionEndReason>,
) {
    let now_active = runtime.session.is_some();
    // `session_started_system` runs first and flips `was_active` to true on
    // the rising edge; we only fire on the falling edge here.
    if !now_active && tracker.started_at.is_some() {
        let duration_s = tracker
            .started_at
            .map(|started| started.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let reason_value = reason.0.take().unwrap_or(SessionEndReason::Disconnect);
        analytics.track(Event::SessionEnded {
            duration_s,
            reason: reason_value,
        });
        tracker.started_at = None;
    }
    let _ = now_active;
}

pub(crate) fn error_relay_system(
    analytics: Res<Analytics>,
    mut events: MessageReader<ClientErrorToast>,
) {
    for event in events.read() {
        analytics.track(Event::Error {
            category: classify_error(&event.text),
        });
    }
}

fn map_screen(screen: Screen) -> ScreenKind {
    match screen {
        Screen::MainMenu => ScreenKind::MainMenu,
        Screen::Options => ScreenKind::Options,
        Screen::Worlds => ScreenKind::Worlds,
        Screen::Multiplayer => ScreenKind::Multiplayer,
        Screen::InGame => ScreenKind::InGame,
    }
}

fn classify_error(text: &str) -> ErrorCategory {
    let lower = text.to_ascii_lowercase();
    if lower.contains("auth") {
        ErrorCategory::Auth
    } else if lower.contains("save") || lower.contains("world") {
        ErrorCategory::Save
    } else if lower.contains("protocol") || lower.contains("version") {
        ErrorCategory::Protocol
    } else {
        ErrorCategory::Network
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::workos::LoginHandle;

    /// A minimal app running just `screen_viewed_system`, so a test can assert
    /// which screen it would record (mirrored in `LastTrackedScreen`) for a
    /// given auth state + menu screen. Analytics is disabled (fire-and-forget
    /// to a worker isn't observable in-process), so `LastTrackedScreen` is the
    /// assertable surface.
    fn screen_app(auth: AuthFlow, screen: Screen) -> App {
        let mut app = App::new();
        app.insert_resource(Analytics::disabled());
        app.insert_resource(auth);
        app.insert_resource(MenuState {
            screen,
            ..Default::default()
        });
        app.insert_resource(LastTrackedScreen::default());
        app.add_systems(Update, screen_viewed_system);
        app
    }

    fn tracked(app: &App) -> Option<ScreenKind> {
        app.world().resource::<LastTrackedScreen>().0
    }

    #[test]
    fn logged_out_records_the_sign_in_screen_not_the_menu_underneath() {
        let mut app = screen_app(AuthFlow::LoggedOut { error: None }, Screen::MainMenu);
        app.update();
        assert_eq!(tracked(&app), Some(ScreenKind::SignIn));
    }

    #[test]
    fn authenticated_records_the_menu_screen() {
        let mut app = screen_app(AuthFlow::Authenticated, Screen::Worlds);
        app.update();
        assert_eq!(tracked(&app), Some(ScreenKind::Worlds));
    }

    #[test]
    fn auth_spinner_records_nothing_so_silent_restore_skips_a_sign_in_view() {
        let (handle, tx) = LoginHandle::pending();
        let mut app = screen_app(AuthFlow::Verifying(handle), Screen::MainMenu);
        app.update();
        assert_eq!(tracked(&app), None);
        drop(tx);
    }

    #[test]
    fn classify_error_recognises_obvious_keywords() {
        assert_eq!(
            classify_error("auth rejected: bad ticket"),
            ErrorCategory::Auth
        );
        assert_eq!(
            classify_error("could not save world: disk full"),
            ErrorCategory::Save
        );
        assert_eq!(
            classify_error("protocol version mismatch"),
            ErrorCategory::Protocol
        );
        assert_eq!(
            classify_error("network error: connection reset"),
            ErrorCategory::Network
        );
    }
}
