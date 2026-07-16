use bevy::prelude::Resource;

use crate::auth::workos::{LoginHandle, WorkosConfig};

/// Resolved WorkOS native-login config, available to the auth UI + systems.
#[derive(Resource)]
pub(crate) struct WorkosAuth(pub(crate) WorkosConfig);

/// Client auth state machine. Gates the title screen: the menu only renders
/// once `Authenticated`. The login splash (`src/app/ui/login.rs`) drives the
/// `LoggedOut → Authenticating` transition; `drive_auth_flow_system` polls the
/// in-flight handles and flips to `Authenticated` (inserting `CurrentUser`).
#[derive(Resource)]
pub(crate) enum AuthFlow {
    /// Startup: a stored session is being silently refreshed (spinner splash).
    Verifying(LoginHandle),
    /// No / invalid session: show the login splash with a sign-in button.
    LoggedOut { error: Option<String> },
    /// Browser sign-in in flight (spinner splash), waiting on the callback.
    Authenticating(LoginHandle),
    /// The sign-in provider could not be reached even after the worker's retry
    /// budget (network trouble or a provider outage; the credentials were
    /// never rejected). The login overlay renders a decision dialog instead of
    /// silently appearing logged out: try the same flow again, fall back to a
    /// fresh browser sign-in, or dismiss to the ordinary splash.
    Unreachable { error: String, retry: AuthRetry },
    /// Signed in; `CurrentUser` is present and the normal menu renders.
    Authenticated,
}

/// What "Try again" on the [`AuthFlow::Unreachable`] dialog re-runs: the flow
/// that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthRetry {
    /// The silent stored-session refresh (boot restore) failed: retry it
    /// silently; the stored refresh token is still on disk and may be fine.
    SilentRestore,
    /// An interactive browser sign-in failed at the token exchange: retry
    /// means a fresh browser round-trip (the old code is single-use).
    BrowserSignIn,
}

impl AuthFlow {
    /// True once the user is signed in and the normal menu may render.
    pub(crate) fn is_authenticated(&self) -> bool {
        matches!(self, Self::Authenticated)
    }

    /// True while a silent restore or an explicit sign-in is still resolving,
    /// i.e. the outcome (menu vs login prompt) is not yet known. The startup
    /// screen holds the opaque loading cover over the 3D backdrop while this is
    /// true, so the menu backdrop never peeks out from behind the "Signing you
    /// in…" splash before auth settles (see `MenuBackdropVisibility::cover_alpha`).
    pub(crate) fn is_in_flight(&self) -> bool {
        matches!(self, Self::Verifying(_) | Self::Authenticating(_))
    }
}
