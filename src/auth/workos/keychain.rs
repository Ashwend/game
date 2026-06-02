//! OS keychain storage for the WorkOS refresh token.
//!
//! The short-lived access token stays in memory; only the long-lived refresh
//! token is persisted, so the next launch can silently re-auth. Backed by the
//! platform-native keystore (macOS Keychain, Windows Credential Manager, Linux
//! kernel keyutils), see the per-OS `keyring` features in `Cargo.toml`.

/// Keychain coordinates for the persisted refresh token.
const KEYRING_SERVICE: &str = "ashwend";
const KEYRING_ACCOUNT: &str = "workos-refresh-token";

fn keyring_entry() -> Option<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).ok()
}

pub(super) fn store_refresh_token(token: &str) {
    if let Some(entry) = keyring_entry() {
        let _ = entry.set_password(token);
    }
}

pub(super) fn load_refresh_token() -> Option<String> {
    keyring_entry()?.get_password().ok()
}

pub(super) fn clear_refresh_token() {
    if let Some(entry) = keyring_entry() {
        let _ = entry.delete_credential();
    }
}
