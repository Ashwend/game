//! WorkOS token endpoint: swap an authorization code (or refresh token) for a
//! session, retry wrapping for provider outages, and the small
//! response-shaping helpers around it.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, SystemTime},
};

use base64::Engine;
use serde::Deserialize;

use crate::auth::account_id_from_sub;

use super::login::Session;

#[derive(Debug, Deserialize)]
pub(super) struct AuthResponse {
    access_token: String,
    refresh_token: String,
    user: WorkosUser,
}

#[derive(Debug, Deserialize)]
struct WorkosUser {
    id: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    first_name: Option<String>,
}

/// Connect / overall deadline for the WorkOS token exchange. The default ureq
/// agent has NO timeouts, so a dead network (sleeping Wi-Fi, captive portal,
/// IPv6 black hole) at boot would hold the "Authenticating" splash for the
/// OS-level TCP timeout, over a minute of spinner. The exchange is one small
/// JSON round trip; if it hasn't answered in this window it isn't going to.
const AUTH_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const AUTH_TIMEOUT: Duration = Duration::from_secs(15);

/// Attempts the sign-in flows (silent restore, browser code exchange) make
/// against a TRANSPORT-shaped failure before giving up and surfacing the
/// outage to the player. Rejections (4xx) never retry: the grant is dead and
/// hammering the provider will not resurrect it.
pub(super) const AUTH_RETRY_ATTEMPTS: u32 = 3;
/// Attempts the pre-connect token renewal makes. Kept lighter than the sign-in
/// flows: the join prompt has its own inline error + retry loop, so a long
/// blocking backoff behind the "Joining server" splash buys little.
pub(super) const AUTH_RENEW_ATTEMPTS: u32 = 2;
/// Base delay between retry attempts; doubles each retry (2 s, then 4 s, ...).
/// Injected into [`retry_auth_call`] so tests can pass zero.
pub(super) const AUTH_RETRY_BACKOFF: Duration = Duration::from_secs(2);
/// Slice length for the backoff sleep, so a raised cancel flag (the player
/// pressing Cancel on the spinner) is honoured promptly mid-backoff.
const RETRY_CANCEL_POLL: Duration = Duration::from_millis(100);

/// Token-endpoint failure, split by whether the provider definitively rejected
/// the grant (the presented token/code is dead) or the call failed in transit
/// (network trouble; a stored refresh token may still be perfectly good).
/// The silent boot-time restore keys off this split: it must not throw away
/// the player's session over a flaky Wi-Fi link.
#[derive(Debug)]
pub(super) enum AuthCallError {
    Rejected(String),
    Transport(String),
}

impl AuthCallError {
    pub(super) fn into_message(self) -> String {
        match self {
            Self::Rejected(message) | Self::Transport(message) => message,
        }
    }

    pub(super) fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected(_))
    }
}

/// POST a grant (`authorization_code` or `refresh_token`) to the WorkOS token
/// endpoint. Public client, no secret; PKCE proves the app's identity.
pub(super) fn post_authenticate(body: serde_json::Value) -> Result<AuthResponse, AuthCallError> {
    ureq::AgentBuilder::new()
        .timeout_connect(AUTH_CONNECT_TIMEOUT)
        .timeout(AUTH_TIMEOUT)
        .build()
        .post(super::config::AUTHENTICATE_URL)
        .send_json(body)
        .map_err(classify_ureq_error)?
        .into_json::<AuthResponse>()
        .map_err(|err| AuthCallError::Transport(format!("unexpected sign-in response: {err}")))
}

/// [`post_authenticate`] wrapped in the standard outage retry policy: up to
/// `attempts` tries, doubling backoff between them, honouring `cancel`.
pub(super) fn post_authenticate_with_retry(
    body: serde_json::Value,
    attempts: u32,
    cancel: Option<&AtomicBool>,
) -> Result<AuthResponse, AuthCallError> {
    retry_auth_call(attempts, AUTH_RETRY_BACKOFF, cancel, || {
        post_authenticate(body.clone())
    })
}

/// Run an auth call with the provider-outage retry policy. Only
/// TRANSPORT-shaped failures (network trouble, timeouts, provider 5xx) retry;
/// a definitive rejection returns immediately (the grant is dead). Between
/// attempts it sleeps a doubling backoff (`backoff`, then 2x, ...), checking
/// `cancel` in short slices so the player's Cancel takes effect promptly. The
/// final transport error is annotated with the attempt count so the log line
/// and the failure dialog both say how hard we tried. Blocking: only call
/// this from the auth worker threads, never a Bevy system.
pub(super) fn retry_auth_call<T>(
    attempts: u32,
    backoff: Duration,
    cancel: Option<&AtomicBool>,
    mut call: impl FnMut() -> Result<T, AuthCallError>,
) -> Result<T, AuthCallError> {
    let attempts = attempts.max(1);
    let mut delay = backoff;
    for attempt in 1..=attempts {
        match call() {
            Ok(value) => return Ok(value),
            Err(error @ AuthCallError::Rejected(_)) => return Err(error),
            Err(AuthCallError::Transport(message)) => {
                let cancelled = cancel.is_some_and(|flag| flag.load(Ordering::Relaxed));
                if attempt == attempts || cancelled {
                    return Err(AuthCallError::Transport(format!(
                        "{message} (after {attempt} attempt{s})",
                        s = if attempt == 1 { "" } else { "s" },
                    )));
                }
                // Backoff, sliced so a raised cancel flag cuts the wait short.
                let mut remaining = delay;
                while remaining > Duration::ZERO {
                    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                        return Err(AuthCallError::Transport(format!(
                            "{message} (cancelled after {attempt} attempt{s})",
                            s = if attempt == 1 { "" } else { "s" },
                        )));
                    }
                    let slice = remaining.min(RETRY_CANCEL_POLL);
                    thread::sleep(slice);
                    remaining = remaining.saturating_sub(slice);
                }
                delay *= 2;
            }
        }
    }
    unreachable!("the loop always returns on its final attempt");
}

pub(super) fn session_from(response: AuthResponse) -> Session {
    let display_name = response
        .user
        .first_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| response.user.email.clone());
    Session {
        account_id: account_id_from_sub(&response.user.id),
        display_name,
        email: response.user.email,
        expires_at: access_token_expiry(&response.access_token),
        access_token: response.access_token,
        refresh_token: response.refresh_token,
    }
}

/// Read `exp` out of the access-token JWT (no verification, the client only
/// needs to know when to refresh; the server does the real verification).
pub(super) fn access_token_expiry(token: &str) -> Option<SystemTime> {
    #[derive(Deserialize)]
    struct Claims {
        exp: u64,
    }
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: Claims = serde_json::from_slice(&bytes).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(claims.exp))
}

fn classify_ureq_error(error: ureq::Error) -> AuthCallError {
    match error {
        ureq::Error::Status(code, response) => {
            let detail = response.into_string().unwrap_or_default();
            // Only a 4xx is the provider saying "this grant is dead". A 5xx is
            // the provider having a bad day, which is transport-shaped: the
            // grant may still be fine.
            if (400..500).contains(&code) {
                AuthCallError::Rejected(format!("sign-in rejected ({code}): {detail}"))
            } else {
                AuthCallError::Transport(format!("sign-in provider error ({code}): {detail}"))
            }
        }
        ureq::Error::Transport(transport) => {
            AuthCallError::Transport(format!("sign-in network error: {transport}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_from_prefers_first_name_then_falls_back_to_email() {
        let with_name = session_from(AuthResponse {
            access_token: "h.e.s".to_owned(),
            refresh_token: "refresh".to_owned(),
            user: WorkosUser {
                id: "user_01ABC".to_owned(),
                email: "player@example.com".to_owned(),
                first_name: Some("  Ada  ".to_owned()),
            },
        });
        assert_eq!(with_name.display_name, "Ada");
        assert_eq!(with_name.email, "player@example.com");
        assert_eq!(with_name.refresh_token, "refresh");
        assert_eq!(with_name.account_id, account_id_from_sub("user_01ABC"));

        let no_name = session_from(AuthResponse {
            access_token: "h.e.s".to_owned(),
            refresh_token: "r".to_owned(),
            user: WorkosUser {
                id: "user_01XYZ".to_owned(),
                email: "fallback@example.com".to_owned(),
                first_name: Some("   ".to_owned()),
            },
        });
        assert_eq!(no_name.display_name, "fallback@example.com");
    }

    #[test]
    fn retry_auth_call_retries_transport_up_to_the_attempt_budget() {
        let mut calls = 0;
        let result: Result<(), _> = retry_auth_call(3, Duration::ZERO, None, || {
            calls += 1;
            Err(AuthCallError::Transport("provider down".to_owned()))
        });
        assert_eq!(calls, 3, "transport failures use the whole budget");
        match result {
            Err(AuthCallError::Transport(message)) => {
                assert!(
                    message.contains("after 3 attempts"),
                    "final error says how hard we tried: {message}"
                );
            }
            other => panic!("expected a transport error, got {other:?}"),
        }
    }

    #[test]
    fn retry_auth_call_succeeds_mid_budget_and_stops() {
        let mut calls = 0;
        let result = retry_auth_call(3, Duration::ZERO, None, || {
            calls += 1;
            if calls < 2 {
                Err(AuthCallError::Transport("blip".to_owned()))
            } else {
                Ok(42)
            }
        });
        assert_eq!(calls, 2, "a success stops the retry loop");
        assert!(matches!(result, Ok(42)));
    }

    #[test]
    fn retry_auth_call_never_retries_a_rejection() {
        let mut calls = 0;
        let result: Result<(), _> = retry_auth_call(3, Duration::ZERO, None, || {
            calls += 1;
            Err(AuthCallError::Rejected("grant is dead".to_owned()))
        });
        assert_eq!(calls, 1, "a 4xx rejection must not hammer the provider");
        assert!(matches!(result, Err(AuthCallError::Rejected(_))));
    }

    #[test]
    fn retry_auth_call_stops_when_cancelled() {
        let cancel = AtomicBool::new(true);
        let mut calls = 0;
        let result: Result<(), _> = retry_auth_call(3, Duration::ZERO, Some(&cancel), || {
            calls += 1;
            Err(AuthCallError::Transport("outage".to_owned()))
        });
        assert_eq!(calls, 1, "a raised cancel flag skips the remaining budget");
        assert!(matches!(result, Err(AuthCallError::Transport(_))));
    }

    #[test]
    fn retry_auth_call_treats_zero_attempts_as_one() {
        let mut calls = 0;
        let _: Result<(), _> = retry_auth_call(0, Duration::ZERO, None, || {
            calls += 1;
            Err(AuthCallError::Transport("x".to_owned()))
        });
        assert_eq!(calls, 1);
    }

    #[test]
    fn access_token_expiry_reads_exp_claim_else_none() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"exp":1700000000,"sub":"user_1"}"#);
        let token = format!("header.{payload}.signature");
        assert_eq!(
            access_token_expiry(&token),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );

        assert_eq!(access_token_expiry("not-a-jwt"), None);
        assert_eq!(access_token_expiry("h.@@@.s"), None);
        let no_exp = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"x"}"#);
        assert_eq!(access_token_expiry(&format!("h.{no_exp}.s")), None);
    }
}
