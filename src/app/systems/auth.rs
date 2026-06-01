use bevy::prelude::*;

use crate::{
    app::state::{AuthFlow, CurrentUser, LoadingSplash, MenuState},
    auth::AuthenticatedUser,
    auth::workos::LoginOutcome,
};

/// Polls the in-flight WorkOS login/refresh handle each frame and advances the
/// auth state machine: on success it installs [`CurrentUser`] and crossfades into
/// the title screen; on failure it drops back to the login splash (surfacing
/// the error only for an explicit sign-in attempt, not a silent refresh).
pub(crate) fn drive_auth_flow_system(
    mut commands: Commands,
    mut auth: ResMut<AuthFlow>,
    mut menu: ResMut<MenuState>,
) {
    // Title-screen account actions.
    if menu.sign_out_requested {
        menu.sign_out_requested = false;
        crate::auth::workos::logout();
        commands.remove_resource::<CurrentUser>();
        *auth = AuthFlow::LoggedOut { error: None };
        return;
    }

    let (outcome, was_explicit) = match &*auth {
        AuthFlow::Verifying(handle) => (handle.poll(), false),
        AuthFlow::Authenticating(handle) => (handle.poll(), true),
        AuthFlow::LoggedOut { .. } | AuthFlow::Authenticated => return,
    };

    match outcome {
        LoginOutcome::Pending => {}
        LoginOutcome::Success(session) => {
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
        LoginOutcome::Failed(error) => {
            *auth = AuthFlow::LoggedOut {
                error: was_explicit.then_some(error),
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
            account_id: 77,
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
        assert_eq!(user.0.account_id, 77);
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
            "bad code".to_owned(),
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
            "expired".to_owned()
        ))));
        app.update();
        match app.world().resource::<AuthFlow>() {
            AuthFlow::LoggedOut { error } => {
                assert!(error.is_none(), "a silent refresh failure stays quiet")
            }
            _ => panic!("expected LoggedOut after a silent restore failure"),
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
