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

// Browser launch is identical to the menu Discord link and the updater's
// download-page fallback, so the per-OS command lives once in
// `crate::util::open_url`. Re-exported here so the login flow keeps calling
// `super::open_url`.
use crate::util::open_url;

pub use config::WorkosConfig;
pub use login::{
    LoginHandle, LoginOutcome, ScreenHint, TokenFreshness, begin_login, begin_restore,
    ensure_fresh_token, has_stored_session, logout,
};
// `Session` and `LoginError` are only referenced by NAME from tests (the login
// system consumes them through `LoginOutcome` without naming the types), so
// the re-exports are test-only to avoid unused-import warnings in the lib
// build.
#[cfg(test)]
pub(crate) use login::{LoginError, Session};
