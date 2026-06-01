//! Browser-based WorkOS login for the desktop client, plus its config.
//!
//! Split by concern: [`config`] resolves the client config (build/TOML/env),
//! [`login`] drives the loopback OAuth round-trip, [`tokens`] talks to the
//! WorkOS token endpoint, [`pkce`] holds the PKCE/encoding helpers, and
//! [`keychain`] persists the refresh token.

mod config;
mod keychain;
mod login;
mod pkce;
mod tokens;

use std::process::Command;

pub use config::WorkosConfig;
pub use login::{
    LoginHandle, LoginOutcome, ScreenHint, begin_login, begin_restore, has_stored_session, logout,
};
// `Session` is only referenced by name from tests (the login system consumes a
// boxed `Session` without naming the type), so the re-export is test-only to
// avoid an unused-import warning in the lib build.
#[cfg(test)]
pub(crate) use login::Session;

/// Open the account page in the system browser. WorkOS has no hosted end-user
/// profile page, so this points at our own site (see
/// [`WorkosConfig::account_url`]), where account management can grow over time.
pub fn open_account_page() {
    let _ = open_url(&WorkosConfig::load().account_url);
}

/// Open `url` in the system browser. Best-effort; errors are surfaced to the
/// caller (the login flow reports them, `open_account_page` ignores them).
fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    command.spawn().map(|_| ())
}
