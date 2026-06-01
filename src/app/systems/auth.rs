use bevy::prelude::*;

use crate::{
    app::state::{AuthFlow, LoadingSplash, MenuState, SteamUser},
    steam::AuthenticatedUser,
    workos_login::LoginOutcome,
};

/// Polls the in-flight WorkOS login/refresh handle each frame and advances the
/// auth state machine: on success it installs [`SteamUser`] and crossfades into
/// the title screen; on failure it drops back to the login splash (surfacing
/// the error only for an explicit sign-in attempt, not a silent refresh).
pub(crate) fn drive_auth_flow_system(
    mut commands: Commands,
    mut auth: ResMut<AuthFlow>,
    mut menu: ResMut<MenuState>,
) {
    // Title-screen account actions.
    if menu.manage_account_requested {
        menu.manage_account_requested = false;
        crate::workos_login::open_account_page();
    }
    if menu.sign_out_requested {
        menu.sign_out_requested = false;
        crate::workos_login::logout();
        commands.remove_resource::<SteamUser>();
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
            commands.insert_resource(SteamUser(AuthenticatedUser {
                steam_id: session.account_id,
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
