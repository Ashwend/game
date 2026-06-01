//! WorkOS client configuration for the desktop login + server JWKS verifier.
//!
//! Resolution order (each step overrides the previous), mirroring
//! [`crate::analytics::config`]:
//! 1. **Build-time defaults** baked in via [`option_env!`]. CI exports the
//!    GitHub Variable `GAME_WORKOS_CLIENT_ID` into the `cargo build` step (see
//!    `.github/workflows/release.yml`), so shipped binaries carry the right
//!    client id with nothing for the player to set.
//! 2. **`workos.local.toml`** at the repo root. Gitignored. Points a local
//!    checkout at a dev WorkOS environment.
//! 3. **Runtime environment variables** (`GAME_WORKOS_*`). Override anything
//!    above for one-off runs or CI.
//!
//! Only the public `client_id` is required (it drives both the OAuth authorize
//! request and the server-side JWKS URL). The OAuth + JWKS endpoints are always
//! `api.workos.com` — the AuthKit *domain* the website uses is not needed here.

use std::{fs, path::Path};

use serde::Deserialize;

pub(super) const AUTHORIZE_URL: &str = "https://api.workos.com/user_management/authorize";
pub(super) const AUTHENTICATE_URL: &str = "https://api.workos.com/user_management/authenticate";

/// WorkOS client id used when nothing overrides it. Public — safe to ship.
/// TODO: swap to the production client id before release.
const DEFAULT_CLIENT_ID: &str = "client_01KSZSFDYP8ZVPE63P94ZWJ3WX";
/// Loopback port the browser is redirected back to. Must be registered as a
/// redirect URI in the WorkOS dashboard: `http://127.0.0.1:8765/callback`.
const DEFAULT_REDIRECT_PORT: u16 = 8765;
/// Where "Manage account" sends the player — WorkOS has no hosted end-user
/// profile page, so this points at our own site.
const DEFAULT_ACCOUNT_URL: &str = "https://ashwend.com";

/// File at the repo root that points a local checkout at a WorkOS environment.
const FILE_NAME: &str = "workos.local.toml";

mod env {
    pub(super) const CLIENT_ID: &str = "GAME_WORKOS_CLIENT_ID";
    pub(super) const REDIRECT_PORT: &str = "GAME_WORKOS_REDIRECT_PORT";
    pub(super) const ACCOUNT_URL: &str = "GAME_WORKOS_ACCOUNT_URL";
}

/// Compile-time fallbacks resolved by [`option_env!`] at `cargo build` time, so
/// whichever `GAME_WORKOS_*` values are exported into the build env get baked
/// in as `&'static str` literals.
mod build {
    pub(super) const CLIENT_ID: Option<&str> = option_env!("GAME_WORKOS_CLIENT_ID");
    pub(super) const REDIRECT_PORT: Option<&str> = option_env!("GAME_WORKOS_REDIRECT_PORT");
    pub(super) const ACCOUNT_URL: Option<&str> = option_env!("GAME_WORKOS_ACCOUNT_URL");
}

/// Resolved WorkOS client configuration. Everything here is public.
#[derive(Debug, Clone)]
pub struct WorkosConfig {
    pub client_id: String,
    pub redirect_port: u16,
    pub account_url: String,
}

impl Default for WorkosConfig {
    fn default() -> Self {
        Self {
            client_id: DEFAULT_CLIENT_ID.to_owned(),
            redirect_port: DEFAULT_REDIRECT_PORT,
            account_url: DEFAULT_ACCOUNT_URL.to_owned(),
        }
    }
}

impl WorkosConfig {
    /// Resolve config by overlaying build-time defaults, `workos.local.toml` at
    /// the current working directory, then the `GAME_WORKOS_*` environment.
    pub fn load() -> Self {
        let repo_root = std::env::current_dir().unwrap_or_else(|_| ".".into());
        Self::load_from(&repo_root)
    }

    pub(crate) fn load_from(repo_root: &Path) -> Self {
        let mut resolved = build_defaults();
        resolved = overlay_file(resolved, repo_root);
        resolved = overlay_env(resolved);
        resolved.into_config()
    }

    pub(super) fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.redirect_port)
    }
}

#[derive(Default, Debug, Deserialize)]
struct RawConfig {
    client_id: Option<String>,
    redirect_port: Option<u16>,
    account_url: Option<String>,
}

#[derive(Default, Debug)]
struct Resolved {
    client_id: Option<String>,
    redirect_port: Option<u16>,
    account_url: Option<String>,
}

impl Resolved {
    fn into_config(self) -> WorkosConfig {
        let defaults = WorkosConfig::default();
        WorkosConfig {
            client_id: self.client_id.unwrap_or(defaults.client_id),
            redirect_port: self.redirect_port.unwrap_or(defaults.redirect_port),
            account_url: self.account_url.unwrap_or(defaults.account_url),
        }
    }
}

impl From<RawConfig> for Resolved {
    fn from(raw: RawConfig) -> Self {
        Self {
            client_id: raw.client_id.filter(|id| !id.is_empty()),
            redirect_port: raw.redirect_port,
            account_url: raw.account_url.filter(|url| !url.is_empty()),
        }
    }
}

fn build_defaults() -> Resolved {
    Resolved {
        client_id: trim_static(build::CLIENT_ID).map(str::to_owned),
        redirect_port: trim_static(build::REDIRECT_PORT).and_then(|value| value.parse().ok()),
        account_url: trim_static(build::ACCOUNT_URL).map(str::to_owned),
    }
}

fn overlay_file(mut base: Resolved, repo_root: &Path) -> Resolved {
    let path = repo_root.join(FILE_NAME);
    let raw = match fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<RawConfig>(&contents) {
            Ok(raw) => raw,
            Err(error) => {
                eprintln!("ignoring malformed {}: {error}", path.display());
                return base;
            }
        },
        Err(_) => return base,
    };
    let from_file: Resolved = raw.into();
    if from_file.client_id.is_some() {
        base.client_id = from_file.client_id;
    }
    if from_file.redirect_port.is_some() {
        base.redirect_port = from_file.redirect_port;
    }
    if from_file.account_url.is_some() {
        base.account_url = from_file.account_url;
    }
    base
}

fn overlay_env(mut base: Resolved) -> Resolved {
    if let Some(value) = env_string(env::CLIENT_ID) {
        base.client_id = Some(value);
    }
    if let Some(value) = env_string(env::REDIRECT_PORT).and_then(|value| value.parse().ok()) {
        base.redirect_port = Some(value);
    }
    if let Some(value) = env_string(env::ACCOUNT_URL) {
        base.account_url = Some(value);
    }
    base
}

fn env_string(key: &str) -> Option<String> {
    let value = std::env::var(key).ok()?.trim().to_owned();
    if value.is_empty() { None } else { Some(value) }
}

/// Trim a build-time `option_env!` value and drop empties so an unset-or-blank
/// variable doesn't show up as a config override.
fn trim_static(value: Option<&'static str>) -> Option<&'static str> {
    let value = value?.trim();
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("game-workos-config-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn missing_file_falls_back_to_baked_defaults() {
        let root = temp_root();
        let config = WorkosConfig::load_from(&root);
        assert_eq!(config.client_id, DEFAULT_CLIENT_ID);
        assert_eq!(config.redirect_port, DEFAULT_REDIRECT_PORT);
        assert_eq!(config.account_url, DEFAULT_ACCOUNT_URL);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn file_overrides_client_id_and_port() {
        let root = temp_root();
        fs::write(
            root.join(FILE_NAME),
            "client_id = \"client_fromfile\"\nredirect_port = 9123\n",
        )
        .unwrap();
        let config = WorkosConfig::load_from(&root);
        assert_eq!(config.client_id, "client_fromfile");
        assert_eq!(config.redirect_port, 9123);
        // Unset field keeps its default.
        assert_eq!(config.account_url, DEFAULT_ACCOUNT_URL);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_file_is_ignored() {
        let root = temp_root();
        fs::write(root.join(FILE_NAME), "client_id = = nope").unwrap();
        let config = WorkosConfig::load_from(&root);
        assert_eq!(config.client_id, DEFAULT_CLIENT_ID);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn redirect_uri_is_loopback_callback() {
        let config = WorkosConfig {
            client_id: "client_x".to_owned(),
            redirect_port: 9000,
            account_url: DEFAULT_ACCOUNT_URL.to_owned(),
        };
        assert_eq!(config.redirect_uri(), "http://127.0.0.1:9000/callback");
    }
}
