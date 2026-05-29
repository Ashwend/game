use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::SteamId;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMode {
    Offline,
    Steam,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthenticatedUser {
    pub steam_id: SteamId,
    pub display_name: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerRegistrationRequest {
    pub name: String,
    pub bind_addr: String,
    pub map: String,
    pub max_players: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerRegistration {
    pub backend: String,
    pub visible_in_server_browser: bool,
    pub detail: String,
}

pub trait SteamBackend: Send + Sync + 'static {
    fn current_user(&self) -> Result<AuthenticatedUser, SteamError>;
    fn open_server_browser(&self) -> Result<(), SteamError>;
    fn register_server(
        &self,
        request: &ServerRegistrationRequest,
    ) -> Result<ServerRegistration, SteamError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct OfflineSteamBackend;

/// Fallback offline identity, used only when no per-install id is available
/// (the `cli` entry points that don't load client settings) and
/// `GAME_STEAM_ID` is unset. The normal client path persists a per-install id
/// instead — see [`generate_install_id`] and [`OfflineSteamBackend::user_for_install`].
const DEFAULT_OFFLINE_STEAM_ID: SteamId = 76_561_197_960_287_930;

impl OfflineSteamBackend {
    /// Build the offline identity for `persisted_id` — the stable per-install
    /// id loaded from client settings. `GAME_STEAM_ID` still overrides it so
    /// `multiplayer-test` can hand each spawned window a distinct identity on
    /// a single machine.
    pub fn user_for_install(&self, persisted_id: SteamId) -> AuthenticatedUser {
        offline_user(env_steam_id().unwrap_or(persisted_id))
    }
}

impl SteamBackend for OfflineSteamBackend {
    fn current_user(&self) -> Result<AuthenticatedUser, SteamError> {
        Ok(offline_user(
            env_steam_id().unwrap_or(DEFAULT_OFFLINE_STEAM_ID),
        ))
    }

    fn open_server_browser(&self) -> Result<(), SteamError> {
        open_steam_uri("steam://open/servers")
    }

    fn register_server(
        &self,
        request: &ServerRegistrationRequest,
    ) -> Result<ServerRegistration, SteamError> {
        Ok(ServerRegistration {
            backend: "offline-dev".to_owned(),
            visible_in_server_browser: false,
            detail: format!(
                "{} listening at {} without Steam master-server registration",
                request.name, request.bind_addr
            ),
        })
    }
}

pub fn offline_auth_token(steam_id: SteamId) -> String {
    format!("offline:{steam_id}")
}

fn env_steam_id() -> Option<SteamId> {
    std::env::var("GAME_STEAM_ID")
        .ok()
        .and_then(|value| value.parse::<SteamId>().ok())
}

fn offline_display_name() -> String {
    std::env::var("GAME_PLAYER_NAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "Player".to_owned())
}

fn offline_user(steam_id: SteamId) -> AuthenticatedUser {
    AuthenticatedUser {
        steam_id,
        display_name: offline_display_name(),
        token: offline_auth_token(steam_id),
    }
}

/// Generate a fresh stable per-installation id. Persisted in client settings
/// on first launch and reused thereafter so the same machine keeps the same
/// player identity (and saved inventory) across sessions. Derived from a v4
/// UUID truncated to the 64-bit [`SteamId`] width; guaranteed non-zero so `0`
/// stays usable as the "not generated yet" sentinel in settings.
pub fn generate_install_id() -> SteamId {
    match uuid::Uuid::new_v4().as_u64_pair().0 {
        0 => 1,
        id => id,
    }
}

pub fn verify_auth_ticket(
    mode: AuthMode,
    steam_id: SteamId,
    token: &str,
) -> Result<(), SteamError> {
    match mode {
        AuthMode::Offline => {
            let expected = offline_auth_token(steam_id);
            if token == expected {
                Ok(())
            } else {
                Err(SteamError::AuthRejected(
                    "offline auth token did not match the claimed Steam id".to_owned(),
                ))
            }
        }
        AuthMode::Steam => verify_steam_ticket(steam_id, token),
    }
}

#[cfg(feature = "steam")]
fn verify_steam_ticket(_steam_id: SteamId, token: &str) -> Result<(), SteamError> {
    if token.trim().is_empty() {
        return Err(SteamError::AuthRejected(
            "Steam auth ticket was empty".to_owned(),
        ));
    }

    Err(SteamError::Unavailable(
        "Steamworks is compiled, but live server-side ticket validation still needs a SteamGameServer verifier"
            .to_owned(),
    ))
}

#[cfg(not(feature = "steam"))]
fn verify_steam_ticket(_steam_id: SteamId, _token: &str) -> Result<(), SteamError> {
    Err(SteamError::Unavailable(
        "Steam auth requires building with --features steam and wiring the Steamworks app id"
            .to_owned(),
    ))
}

fn open_steam_uri(uri: &str) -> Result<(), SteamError> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(uri);
        command
    };

    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(uri);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", uri]);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| SteamError::Unavailable(format!("could not open Steam: {error}")))
}

#[derive(Debug, Error)]
pub enum SteamError {
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    AuthRejected(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_auth_matches_claimed_id() {
        assert!(verify_auth_ticket(AuthMode::Offline, 42, &offline_auth_token(42)).is_ok());
        assert!(verify_auth_ticket(AuthMode::Offline, 42, &offline_auth_token(7)).is_err());
    }

    #[test]
    fn offline_auth_rejects_wildcard_singleplayer_token() {
        // The literal "singleplayer" string used to be a wildcard that
        // accepted any claimed Steam id under AuthMode::Offline. If a dedicated
        // server were ever launched in Offline mode, a malicious client could
        // claim any identity with it. Verify the shortcut stays rejected.
        assert!(verify_auth_ticket(AuthMode::Offline, 42, "singleplayer").is_err());
    }
}
