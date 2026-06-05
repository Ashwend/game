//! Browser-based WorkOS login for the desktop client, plus its config.
//!
//! Split by concern: [`config`] resolves the client config (build/TOML/env),
//! [`login`] drives the loopback OAuth round-trip, [`tokens`] talks to the
//! WorkOS token endpoint, [`pkce`] holds the PKCE/encoding helpers, and
//! [`token_store`] persists the refresh token (sealed on disk).

mod config;
mod login;
mod pkce;
mod token_store;
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

/// Open `url` in the system browser. Best-effort; the login flow surfaces any
/// error to the user.
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
