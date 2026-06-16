//! Shared platform-directory identity.
//!
//! The `(qualifier, organization, application)` triple decides where the OS
//! places our per-user data and config directories. It lives in exactly one
//! place because a wrong value here would silently relocate every user file:
//! world saves, client settings, the sealed WorkOS refresh token, the
//! analytics distinct id, and the log file. All of those resolve through
//! [`project_dirs`].

use directories::ProjectDirs;

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "Ashwend";
const APPLICATION: &str = "Ashwend";

/// Resolve the OS-specific project directories. `None` only on platforms with
/// no notion of a home directory (per the `directories` crate).
pub fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_dirs_uses_the_ashwend_identity() {
        // On any host with a home dir, the data dir must sit under the Ashwend
        // application folder. This pins the identity so an accidental edit
        // cannot relocate user files. Case-insensitive because the `directories`
        // crate lowercases the application name for Linux XDG paths
        // (`~/.local/share/ashwend`) while macOS keeps it as `Ashwend`.
        if let Some(dirs) = project_dirs() {
            let data = dirs.data_dir().to_string_lossy().to_lowercase();
            assert!(
                data.contains("ashwend"),
                "data dir should live under the Ashwend identity, got {data}"
            );
        }
    }
}
