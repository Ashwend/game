//! Player identity types shared by the client and the authoritative server.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::protocol::AccountId;

/// Fallback account id for a lone `--connect` launch with no `GAME_ACCOUNT_ID`
/// set. A distinctive value so it's obvious in logs/saves that no real identity
/// was injected.
const DEFAULT_BYPASS_ACCOUNT_ID: AccountId = 76_561_197_960_287_930;

/// Identity a signed-in client carries on the `Auth` handshake. `account_id`
/// keys every authoritative map and the save format; `token` is the WorkOS
/// access token for a real login (empty for the local no-auth bypass).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthenticatedUser {
    pub account_id: AccountId,
    pub display_name: String,
    pub token: String,
}

/// Identity the server admits a client under once their `Auth` handshake checks
/// out. `account_id` is what every authoritative map and the save format key
/// on; `display_name` is set when the provider carries one (WorkOS may), else
/// the server falls back to the client-supplied name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIdentity {
    pub account_id: AccountId,
    pub display_name: Option<String>,
}

/// Derive a stable 64-bit account id from a WorkOS subject (`sub`, e.g.
/// `user_01…`). Truncated SHA-256 keeps the `u64`-keyed identity maps and the
/// on-disk save format byte-compatible; non-zero so `0` stays the "unset"
/// sentinel. Distinct subjects collide only on a 64-bit hash clash — negligible
/// at playtest scale.
pub fn account_id_from_sub(sub: &str) -> AccountId {
    let digest = Sha256::digest(sub.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    match u64::from_be_bytes(bytes) {
        0 => 1,
        id => id,
    }
}

/// Build the bypass identity for the `--connect` test path (used by
/// `./cli multiplayer-test`). Reads `GAME_ACCOUNT_ID` and `GAME_PLAYER_NAME`
/// from the environment so each spawned window gets a distinct, deterministic
/// player. The token is empty — the loopback/localhost server admits these
/// under [`crate::auth::AuthMode::NoAuth`], which trusts the claimed identity.
pub fn bypass_identity_from_env() -> AuthenticatedUser {
    let account_id = std::env::var("GAME_ACCOUNT_ID")
        .ok()
        .and_then(|value| value.parse::<AccountId>().ok())
        .filter(|&id| id != 0)
        .unwrap_or(DEFAULT_BYPASS_ACCOUNT_ID);
    let display_name = std::env::var("GAME_PLAYER_NAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "Player".to_owned());
    AuthenticatedUser {
        account_id,
        display_name,
        token: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id_from_sub_is_stable_distinct_and_nonzero() {
        let id = account_id_from_sub("user_01ABC");
        assert_eq!(id, account_id_from_sub("user_01ABC"));
        assert_ne!(id, account_id_from_sub("user_01XYZ"));
        assert_ne!(id, 0);
    }

    #[test]
    fn bypass_identity_token_is_empty() {
        // Whatever id resolves (env or default), the bypass identity carries no
        // token — the local NoAuth server trusts the claim, not a token.
        let user = bypass_identity_from_env();
        assert!(user.token.is_empty());
        assert!(!user.display_name.is_empty());
    }
}
