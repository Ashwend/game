//! WorkOS token endpoint: swap an authorization code (or refresh token) for a
//! session, and the small response-shaping helpers around it.

use std::time::{Duration, SystemTime};

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

/// POST a grant (`authorization_code` or `refresh_token`) to the WorkOS token
/// endpoint. Public client, no secret; PKCE proves the app's identity.
pub(super) fn post_authenticate(body: serde_json::Value) -> Result<AuthResponse, String> {
    ureq::post(super::config::AUTHENTICATE_URL)
        .send_json(body)
        .map_err(describe_ureq_error)?
        .into_json::<AuthResponse>()
        .map_err(|err| format!("unexpected sign-in response: {err}"))
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
fn access_token_expiry(token: &str) -> Option<SystemTime> {
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

fn describe_ureq_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let detail = response.into_string().unwrap_or_default();
            format!("sign-in rejected ({code}): {detail}")
        }
        ureq::Error::Transport(transport) => format!("sign-in network error: {transport}"),
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
