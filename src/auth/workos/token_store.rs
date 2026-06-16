//! Encrypted on-disk store for the WorkOS refresh token.
//!
//! Replaces the OS keychain. The platform keystores (macOS Keychain, Windows
//! Credential Manager, Linux keyutils) prompt the user and read as a security
//! interruption every launch. Instead we keep the long-lived refresh token in a
//! small file under the platform config dir, sealed with [`crate::local_crypto`]
//! so it isn't plain text on disk. The short-lived access token still lives in
//! memory only.
//!
//! This is not a hardened secret store (the seal key is in the binary); it just
//! keeps the token out of plain text where a file or process scan could lift
//! it. A token that can't be read (missing, tampered, or written by an older
//! build) simply reads back as "no session", so the client falls back to the
//! login splash and the player signs in again.

use std::{fs, path::PathBuf};

use crate::local_crypto;

/// Config-file name. Sits next to `settings.dat` in the same platform config
/// directory (both resolve through `crate::util::platform::project_dirs`).
const TOKEN_FILE: &str = "session.bin";

fn token_path() -> Option<PathBuf> {
    crate::util::platform::project_dirs().map(|dirs| dirs.config_dir().join(TOKEN_FILE))
}

pub(super) fn store_refresh_token(token: &str) {
    let Some(path) = token_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, local_crypto::seal(token.as_bytes()));
}

pub(super) fn load_refresh_token() -> Option<String> {
    let path = token_path()?;
    let sealed = fs::read(&path).ok()?;
    let plaintext = local_crypto::open(&sealed)?;
    String::from_utf8(plaintext).ok()
}

pub(super) fn clear_refresh_token() {
    if let Some(path) = token_path() {
        let _ = fs::remove_file(path);
    }
}
