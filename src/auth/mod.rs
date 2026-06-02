//! Player authentication.
//!
//! Two modes (see [`AuthMode`]):
//! - [`AuthMode::Workos`], the real provider. The client presents a WorkOS
//!   access-token JWT, which the server verifies offline against the WorkOS
//!   JWKS (see [`WorkosVerifier`]). The default for any dedicated server.
//! - [`AuthMode::NoAuth`], loopback/localhost only. The server trusts the
//!   client's claimed account id and display name with no token check. Used by
//!   singleplayer (the in-process loopback host) and `./cli multiplayer-test`.
//!
//! The browser-based WorkOS login the desktop client drives lives in
//! [`workos`]; the server-side verifier lives in [`verify`]; the shared
//! identity types live in [`identity`].

mod identity;
mod verify;
pub(crate) mod workos;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::AccountId;

pub use identity::{
    AuthenticatedUser, VerifiedIdentity, account_id_from_sub, bypass_identity_from_env,
};
pub use verify::WorkosVerifier;

/// How the authoritative server validates a connecting client's identity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMode {
    /// Real identity: verify the client's WorkOS access-token JWT against the
    /// WorkOS JWKS (see [`WorkosVerifier`]). The default for dedicated servers.
    Workos,
    /// Local-only: trust the client's claimed account id and display name with
    /// no token check. Only ever correct for an in-process loopback host
    /// (singleplayer) or a localhost `multiplayer-test` server, never expose a
    /// `NoAuth` server to the network, or a client could claim any identity.
    NoAuth,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    AuthRejected(String),
}

/// Validate a client's auth handshake and resolve the identity to admit them
/// under. For [`AuthMode::Workos`] the identity comes from the *verified*
/// token, never the client's claim; [`AuthMode::NoAuth`] trusts the claim
/// (`token` is ignored).
pub fn authenticate(
    mode: AuthMode,
    workos: Option<&WorkosVerifier>,
    claimed_id: AccountId,
    token: &str,
) -> Result<VerifiedIdentity, AuthError> {
    match mode {
        AuthMode::NoAuth => Ok(VerifiedIdentity {
            account_id: claimed_id,
            display_name: None,
        }),
        AuthMode::Workos => {
            let verifier = workos.ok_or_else(|| {
                AuthError::Unavailable(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_auth_admits_the_claimed_identity() {
        let identity =
            authenticate(AuthMode::NoAuth, None, 42, "").expect("NoAuth trusts the claimed id");
        assert_eq!(identity.account_id, 42);
        // Display name comes from the client-supplied name, not the token.
        assert!(identity.display_name.is_none());
    }

    #[test]
    fn workos_mode_without_a_verifier_is_rejected() {
        assert!(authenticate(AuthMode::Workos, None, 0, "any.jwt.here").is_err());
    }

    #[test]
    fn auth_mode_round_trips_through_serde() {
        for mode in [AuthMode::Workos, AuthMode::NoAuth] {
            let json = serde_json::to_string(&mode).expect("serialize");
            let parsed: AuthMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(mode, parsed);
        }
    }
}
