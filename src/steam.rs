use std::{
    process::Command,
    sync::Mutex,
    time::{Duration, Instant},
};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::SteamId;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMode {
    /// Legacy/dev: the client claims its own id and proves it with the
    /// matching `offline:<id>` token. Removed once WorkOS login lands.
    Offline,
    /// Legacy Steamworks ticket validation. Slated for removal.
    Steam,
    /// Real identity: the client presents a WorkOS access-token JWT, which the
    /// server verifies offline against the WorkOS JWKS (see [`WorkosVerifier`]).
    Workos,
    /// Local testing only (`./cli multiplayer-test`): accepts a synthetic
    /// `test:<id>` token so spawned windows get deterministic identities with no
    /// browser round-trip. Never the default for a dedicated server.
    Test,
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

/// Identity the server admits a client under once their `Auth` handshake checks
/// out. `account_id` is what every authoritative map and the save format key on;
/// `display_name` is set when the provider carries one (WorkOS may), else the
/// server falls back to the client-supplied name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIdentity {
    pub account_id: SteamId,
    pub display_name: Option<String>,
}

/// Validate a client's auth handshake and resolve the identity to admit them
/// under. For [`AuthMode::Workos`]/[`AuthMode::Test`] the identity comes from
/// the *verified* token, never the client's claim; `claimed_id` is only used by
/// the legacy offline/Steam paths.
pub fn authenticate(
    mode: AuthMode,
    workos: Option<&WorkosVerifier>,
    claimed_id: SteamId,
    token: &str,
) -> Result<VerifiedIdentity, SteamError> {
    match mode {
        AuthMode::Offline => {
            if token == offline_auth_token(claimed_id) {
                Ok(VerifiedIdentity {
                    account_id: claimed_id,
                    display_name: None,
                })
            } else {
                Err(SteamError::AuthRejected(
                    "offline auth token did not match the claimed id".to_owned(),
                ))
            }
        }
        AuthMode::Steam => {
            verify_steam_ticket(claimed_id, token)?;
            Ok(VerifiedIdentity {
                account_id: claimed_id,
                display_name: None,
            })
        }
        AuthMode::Test => {
            let account_id = parse_test_token(token).ok_or_else(|| {
                SteamError::AuthRejected("test auth token was not `test:<id>`".to_owned())
            })?;
            Ok(VerifiedIdentity {
                account_id,
                display_name: None,
            })
        }
        AuthMode::Workos => {
            let verifier = workos.ok_or_else(|| {
                SteamError::Unavailable(
                    "Workos auth mode needs a configured WorkOS verifier".to_owned(),
                )
            })?;
            let claims = verifier.verify(token)?;
            Ok(VerifiedIdentity {
                account_id: account_id_from_sub(&claims.sub),
                display_name: claims.name,
            })
        }
    }
}

/// Synthetic token for [`AuthMode::Test`]; pairs with [`parse_test_token`].
/// Used by `multiplayer-test` once it moves onto `Test` mode; kept available
/// now so the mode has a matching producer.
#[allow(dead_code)]
pub fn test_auth_token(account_id: SteamId) -> String {
    format!("test:{account_id}")
}

fn parse_test_token(token: &str) -> Option<SteamId> {
    token
        .strip_prefix("test:")
        .and_then(|id| id.parse::<SteamId>().ok())
        .filter(|&id| id != 0)
}

/// Derive a stable 64-bit account id from a WorkOS subject (`sub`, e.g.
/// `user_01…`). Truncated SHA-256 keeps the `u64`-keyed identity maps and the
/// on-disk save format byte-compatible; non-zero so `0` stays the "unset"
/// sentinel. Distinct subjects collide only on a 64-bit hash clash — negligible
/// at playtest scale.
pub fn account_id_from_sub(sub: &str) -> SteamId {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(sub.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    match u64::from_be_bytes(bytes) {
        0 => 1,
        id => id,
    }
}

/// Minimum gap between JWKS refetches, so a client spamming tokens with bogus
/// `kid`s can't make the server hammer WorkOS.
const JWKS_MIN_REFRESH: Duration = Duration::from_secs(60);
/// How long a fetched JWKS is trusted before a proactive refresh.
const JWKS_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// Offline verifier for WorkOS access-token JWTs. Validates RS256 signatures
/// against the public JWKS for one client id — no API key, no secrets. Build
/// once per server and share via `Arc`; the JWKS is fetched lazily and cached.
#[derive(Debug)]
pub struct WorkosVerifier {
    jwks_url: String,
    validation: Validation,
    cache: Mutex<JwksCache>,
}

#[derive(Debug, Default)]
struct JwksCache {
    keys: Option<JwkSet>,
    fetched_at: Option<Instant>,
}

#[derive(Debug, Deserialize)]
struct WorkosClaims {
    sub: String,
    #[serde(default)]
    name: Option<String>,
}

impl WorkosVerifier {
    pub fn new(client_id: &str) -> Self {
        // Signature + expiry are the hard gates. Binding to this client's JWKS
        // already ties the token to this WorkOS app; issuer/audience checks stay
        // off until confirmed against a live token (see docs/networking.md).
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = true;
        validation.validate_aud = false;
        validation.leeway = 30;
        Self {
            jwks_url: format!("https://api.workos.com/sso/jwks/{client_id}"),
            validation,
            cache: Mutex::new(JwksCache::default()),
        }
    }

    fn verify(&self, token: &str) -> Result<WorkosClaims, SteamError> {
        let header = decode_header(token)
            .map_err(|err| SteamError::AuthRejected(format!("malformed access token: {err}")))?;
        let kid = header
            .kid
            .ok_or_else(|| SteamError::AuthRejected("access token had no key id".to_owned()))?;
        let key = self.decoding_key(&kid)?;
        let data = decode::<WorkosClaims>(token, &key, &self.validation)
            .map_err(|err| SteamError::AuthRejected(format!("access token rejected: {err}")))?;
        Ok(data.claims)
    }

    fn decoding_key(&self, kid: &str) -> Result<DecodingKey, SteamError> {
        if let Some(key) = self.cached_key(kid)? {
            return Ok(key);
        }
        self.refresh_jwks()?;
        self.cached_key(kid)?
            .ok_or_else(|| SteamError::AuthRejected("unknown access-token signing key".to_owned()))
    }

    fn cached_key(&self, kid: &str) -> Result<Option<DecodingKey>, SteamError> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| SteamError::Unavailable("JWKS cache lock poisoned".to_owned()))?;
        if cache
            .fetched_at
            .is_none_or(|at| at.elapsed() >= JWKS_MAX_AGE)
        {
            return Ok(None);
        }
        let Some(jwk) = cache.keys.as_ref().and_then(|set| set.find(kid)) else {
            return Ok(None);
        };
        DecodingKey::from_jwk(jwk)
            .map(Some)
            .map_err(|err| SteamError::Unavailable(format!("bad JWKS key: {err}")))
    }

    fn refresh_jwks(&self) -> Result<(), SteamError> {
        {
            let cache = self
                .cache
                .lock()
                .map_err(|_| SteamError::Unavailable("JWKS cache lock poisoned".to_owned()))?;
            if cache
                .fetched_at
                .is_some_and(|at| at.elapsed() < JWKS_MIN_REFRESH)
            {
                return Ok(());
            }
        }
        let body = ureq::get(&self.jwks_url)
            .call()
            .map_err(|err| SteamError::Unavailable(format!("could not fetch JWKS: {err}")))?
            .into_string()
            .map_err(|err| SteamError::Unavailable(format!("could not read JWKS: {err}")))?;
        let keys: JwkSet = serde_json::from_str(&body)
            .map_err(|err| SteamError::Unavailable(format!("malformed JWKS: {err}")))?;
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| SteamError::Unavailable("JWKS cache lock poisoned".to_owned()))?;
        cache.keys = Some(keys);
        cache.fetched_at = Some(Instant::now());
        Ok(())
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
        assert!(authenticate(AuthMode::Offline, None, 42, &offline_auth_token(42)).is_ok());
        assert!(authenticate(AuthMode::Offline, None, 42, &offline_auth_token(7)).is_err());
    }

    #[test]
    fn offline_auth_rejects_wildcard_singleplayer_token() {
        // The literal "singleplayer" string used to be a wildcard that
        // accepted any claimed id under AuthMode::Offline. If a dedicated
        // server were ever launched in Offline mode, a malicious client could
        // claim any identity with it. Verify the shortcut stays rejected.
        assert!(authenticate(AuthMode::Offline, None, 42, "singleplayer").is_err());
    }

    #[test]
    fn test_mode_derives_identity_from_token() {
        let identity = authenticate(AuthMode::Test, None, 0, &test_auth_token(7))
            .expect("a well-formed test token is accepted");
        assert_eq!(identity.account_id, 7);
        // The claimed id is ignored — the token is the source of truth.
        assert!(authenticate(AuthMode::Test, None, 999, "garbage").is_err());
        assert!(authenticate(AuthMode::Test, None, 0, "test:0").is_err());
    }

    #[test]
    fn workos_mode_without_a_verifier_is_rejected() {
        assert!(authenticate(AuthMode::Workos, None, 0, "any.jwt.here").is_err());
    }

    #[test]
    fn account_id_from_sub_is_stable_distinct_and_nonzero() {
        let id = account_id_from_sub("user_01ABC");
        assert_eq!(id, account_id_from_sub("user_01ABC"));
        assert_ne!(id, account_id_from_sub("user_01XYZ"));
        assert_ne!(id, 0);
    }
}
