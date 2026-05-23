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

impl SteamBackend for OfflineSteamBackend {
    fn current_user(&self) -> Result<AuthenticatedUser, SteamError> {
        let steam_id = std::env::var("GAME_STEAM_ID")
            .ok()
            .and_then(|value| value.parse::<SteamId>().ok())
            .unwrap_or(76_561_197_960_287_930);
        let display_name = std::env::var("GAME_PLAYER_NAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "Player".to_owned());

        Ok(AuthenticatedUser {
            steam_id,
            display_name,
            token: offline_auth_token(steam_id),
        })
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
