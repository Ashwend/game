use bevy::prelude::*;

use crate::{
    analytics::{Analytics, Event},
    app::state::{AuthFlow, AuthRetry, CurrentUser, LoadingSplash, MenuState},
    auth::AuthenticatedUser,
    auth::workos::LoginOutcome,
};

/// Polls the in-flight WorkOS login/refresh handle each frame and advances the
/// auth state machine: on success it installs [`CurrentUser`] and crossfades
/// into the title screen. Failures fork on the worker's classification: a
/// TRANSIENT failure (the provider was unreachable even after the in-worker
/// retry budget) lands in [`AuthFlow::Unreachable`], whose dialog lets the
/// player decide the next step, so a provider outage never silently presents
/// as "you are logged out"; every other failure drops back to the login splash
/// (surfacing the error text only for an explicit sign-in attempt, not a
/// silent refresh).
///
/// This poller also owns the auth lifecycle LOGGING (in flight, resolved, with
/// durations off `LoginHandle::started`): the silent restore's worker thread
/// starts before the tracing subscriber exists, so anything it logged itself
/// would be dropped, and a player report of "stuck on Authenticating" would
/// again be undiagnosable from ashwend.log.
pub(crate) fn drive_auth_flow_system(
    mut commands: Commands,
    analytics: Res<Analytics>,
    mut auth: ResMut<AuthFlow>,
    mut menu: ResMut<MenuState>,
    mut restore_marker_logged: Local<bool>,
) {
    // Title-screen account actions.
    if menu.sign_out_requested {
        menu.sign_out_requested = false;
        analytics.track(Event::SignedOut);
        crate::auth::workos::logout();
        commands.remove_resource::<CurrentUser>();
        *auth = AuthFlow::LoggedOut { error: None };
        return;
    }

    // A join gave up because the stored session couldn't be renewed. Same
    // teardown as an explicit sign-out, but the login splash carries the reason
    // so the player knows their session lapsed rather than being bounced for no
    // visible cause.
    if let Some(reason) = menu.force_sign_out.take() {
        crate::auth::workos::logout();
        commands.remove_resource::<CurrentUser>();
        *auth = AuthFlow::LoggedOut {
            error: Some(reason),
        };
        return;
    }

    // The player escaped the sign-in wait (closed the browser, changed their
    // mind, or relaunched the app). Tell the worker to stop holding the
    // loopback listener, then drop back to the login splash with no error so
    // they can retry cleanly.
    if menu.cancel_auth_requested {
        menu.cancel_auth_requested = false;
        let in_flight = matches!(*auth, AuthFlow::Authenticating(_) | AuthFlow::Verifying(_));
        if let AuthFlow::Authenticating(handle) | AuthFlow::Verifying(handle) = &*auth {
            handle.cancel();
        }
        if in_flight {
            *auth = AuthFlow::LoggedOut { error: None };
        }
        return;
    }

    if !*restore_marker_logged && matches!(*auth, AuthFlow::Verifying(_)) {
        // One boot-time marker so a restore that never resolves (dead network,
        // provider outage) still leaves a trace: this line with no matching
        // "restored/failed" line after it means the refresh call is stuck.
        *restore_marker_logged = true;
        info!("auth: silent session restore in flight");
    }

    let (outcome, was_explicit, elapsed_seconds) = match &*auth {
        AuthFlow::Verifying(handle) => (handle.poll(), false, handle.started().elapsed()),
        AuthFlow::Authenticating(handle) => (handle.poll(), true, handle.started().elapsed()),
        AuthFlow::LoggedOut { .. } | AuthFlow::Unreachable { .. } | AuthFlow::Authenticated => {
            return;
        }
    };
    let elapsed_seconds = elapsed_seconds.as_secs_f32();
    let flow = if was_explicit {
        "sign-in"
    } else {
        "session restore"
    };

    match outcome {
        LoginOutcome::Pending => {}
        LoginOutcome::Success(session) => {
            info!("auth: {flow} succeeded in {elapsed_seconds:.2}s");
            let session = *session;
            commands.insert_resource(CurrentUser(AuthenticatedUser {
                account_id: session.account_id,
                display_name: session.display_name,
                token: session.access_token,
            }));
            *auth = AuthFlow::Authenticated;
            // Reuse the startup splash so the menu reveal is a crossfade, not a
            // hard cut, once the user is in.
            menu.loading_splash = Some(LoadingSplash::startup());
        }
        LoginOutcome::Failed(error) if error.transient => {
            // The provider was unreachable even after the worker's retries:
            // the stored credentials were never rejected, so put the decision
            // to the player instead of silently appearing logged out.
            warn!(
                "auth: {flow} could not reach the provider after {elapsed_seconds:.2}s: {}",
                error.message
            );
            *auth = AuthFlow::Unreachable {
                error: error.message,
                retry: if was_explicit {
                    AuthRetry::BrowserSignIn
                } else {
                    AuthRetry::SilentRestore
                },
            };
        }
        LoginOutcome::Failed(error) => {
            warn!(
                "auth: {flow} failed after {elapsed_seconds:.2}s: {}",
                error.message
            );
            *auth = AuthFlow::LoggedOut {
                error: was_explicit.then_some(error.message),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        app::state::{AuthFlow, CurrentUser, LoadingSplashKind, MenuState},
        auth::workos::{LoginHandle, Session},
    };

    fn session() -> Session {
        Session {
            account_id: crate::protocol::AccountId(77),
            display_name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
            access_token: "access".to_owned(),
            refresh_token: "refresh".to_owned(),
            expires_at: None,
        }
    }

    /// A bare app with the auth system installed and the startup splash cleared,
    /// so a test can assert the system is what (re)installs it.
    fn app_with(auth: AuthFlow) -> App {
        let mut app = App::new();
        app.insert_resource(auth);
        app.insert_resource(Analytics::disabled());
        app.insert_resource(MenuState {
            loading_splash: None,
            ..Default::default()
        });
        app.add_systems(Update, drive_auth_flow_system);
        app
    }

    #[test]
    fn successful_login_installs_user_and_crossfades_in() {
        let mut app = app_with(AuthFlow::Authenticating(LoginHandle::ready(Ok(session()))));
        app.update();

        let user = app
            .world()
            .get_resource::<CurrentUser>()
            .expect("a successful login installs the signed-in user");
        assert_eq!(user.0.account_id, crate::protocol::AccountId(77));
        assert_eq!(user.0.display_name, "Ada");
        assert_eq!(user.0.token, "access");

        assert!(matches!(
            app.world().resource::<AuthFlow>(),
            AuthFlow::Authenticated
        ));
        let splash_kind = app
            .world()
            .resource::<MenuState>()
            .loading_splash
            .as_ref()
            .map(|splash| splash.kind);
        assert_eq!(
            splash_kind,
            Some(LoadingSplashKind::Startup),
            "login should reuse the startup splash for a crossfade reveal"
        );
    }

    #[test]
    fn explicit_sign_in_failure_surfaces_the_error() {
        let mut app = app_with(AuthFlow::Authenticating(LoginHandle::ready(Err(
            crate::auth::workos::LoginError::test_local("bad code"),
        ))));
        app.update();
        match app.world().resource::<AuthFlow>() {
            AuthFlow::LoggedOut { error } => assert_eq!(error.as_deref(), Some("bad code")),
            _ => panic!("expected LoggedOut after an explicit failure"),
        }
    }

    #[test]
    fn silent_restore_failure_returns_to_login_without_an_error() {
        let mut app = app_with(AuthFlow::Verifying(LoginHandle::ready(Err(
            crate::auth::workos::LoginError::test_local("expired"),
        ))));
        app.update();
        match app.world().resource::<AuthFlow>() {
            AuthFlow::LoggedOut { error } => {
                assert!(error.is_none(), "a silent refresh failure stays quiet");
            }
            _ => panic!("expected LoggedOut after a silent restore failure"),
        }
    }

    #[test]
    fn transient_restore_failure_opens_the_unreachable_dialog() {
        // A provider outage during the boot restore must NOT silently present
        // as logged out: the player gets the decision dialog, with retry wired
        // to re-run the silent restore (the stored token may be fine).
        let mut app = app_with(AuthFlow::Verifying(LoginHandle::ready(Err(
            crate::auth::workos::LoginError::test_transient("sign-in provider error (503)"),
        ))));
        app.update();
        match app.world().resource::<AuthFlow>() {
            AuthFlow::Unreachable { error, retry } => {
                assert!(error.contains("503"));
                assert_eq!(*retry, crate::app::state::AuthRetry::SilentRestore);
            }
            _ => panic!("expected Unreachable after a transient restore failure"),
        }
        assert!(
            app.world().get_resource::<CurrentUser>().is_none(),
            "no user is installed on failure"
        );
    }

    #[test]
    fn transient_sign_in_failure_opens_the_unreachable_dialog_with_browser_retry() {
        let mut app = app_with(AuthFlow::Authenticating(LoginHandle::ready(Err(
            crate::auth::workos::LoginError::test_transient("sign-in network error"),
        ))));
        app.update();
        match app.world().resource::<AuthFlow>() {
            AuthFlow::Unreachable { retry, .. } => {
                assert_eq!(*retry, crate::app::state::AuthRetry::BrowserSignIn);
            }
            _ => panic!("expected Unreachable after a transient sign-in failure"),
        }
    }

    #[test]
    fn pending_login_stays_in_flight() {
        let (handle, tx) = LoginHandle::pending();
        let mut app = app_with(AuthFlow::Authenticating(handle));
        app.update();
        assert!(matches!(
            app.world().resource::<AuthFlow>(),
            AuthFlow::Authenticating(_)
        ));
        // Keep the sender alive until after the poll so the channel doesn't
        // disconnect and report a spurious failure.
        drop(tx);
    }
}
