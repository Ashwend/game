//! Server-side WorkOS access-token verification.
//!
//! Validates RS256 access-token JWTs against the public JWKS for one client id
//! — no API key, no secrets. Build once per server and share via `Arc`; the
//! JWKS is fetched lazily over HTTP and cached.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;

use super::AuthError;

/// Minimum gap between JWKS refetches, so a client spamming tokens with bogus
/// `kid`s can't make the server hammer WorkOS.
const JWKS_MIN_REFRESH: Duration = Duration::from_secs(60);
/// How long a fetched JWKS is trusted before a proactive refresh.
const JWKS_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// Offline verifier for WorkOS access-token JWTs.
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

/// The subset of WorkOS access-token claims the server consumes.
#[derive(Debug, Deserialize)]
pub(super) struct WorkosClaims {
    pub(super) sub: String,
    #[serde(default)]
    pub(super) name: Option<String>,
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

    pub(super) fn verify(&self, token: &str) -> Result<WorkosClaims, AuthError> {
        let header = decode_header(token)
            .map_err(|err| AuthError::AuthRejected(format!("malformed access token: {err}")))?;
        let kid = header
            .kid
            .ok_or_else(|| AuthError::AuthRejected("access token had no key id".to_owned()))?;
        let key = self.decoding_key(&kid)?;
        let data = decode::<WorkosClaims>(token, &key, &self.validation)
            .map_err(|err| AuthError::AuthRejected(format!("access token rejected: {err}")))?;
        Ok(data.claims)
    }

    fn decoding_key(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        if let Some(key) = self.cached_key(kid)? {
            return Ok(key);
        }
        self.refresh_jwks()?;
        self.cached_key(kid)?
            .ok_or_else(|| AuthError::AuthRejected("unknown access-token signing key".to_owned()))
    }

    fn cached_key(&self, kid: &str) -> Result<Option<DecodingKey>, AuthError> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| AuthError::Unavailable("JWKS cache lock poisoned".to_owned()))?;
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
            .map_err(|err| AuthError::Unavailable(format!("bad JWKS key: {err}")))
    }

    fn refresh_jwks(&self) -> Result<(), AuthError> {
        {
            let cache = self
                .cache
                .lock()
                .map_err(|_| AuthError::Unavailable("JWKS cache lock poisoned".to_owned()))?;
            if cache
                .fetched_at
                .is_some_and(|at| at.elapsed() < JWKS_MIN_REFRESH)
            {
                return Ok(());
            }
        }
        let body = ureq::get(&self.jwks_url)
            .call()
            .map_err(|err| AuthError::Unavailable(format!("could not fetch JWKS: {err}")))?
            .into_string()
            .map_err(|err| AuthError::Unavailable(format!("could not read JWKS: {err}")))?;
        let keys: JwkSet = serde_json::from_str(&body)
            .map_err(|err| AuthError::Unavailable(format!("malformed JWKS: {err}")))?;
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| AuthError::Unavailable("JWKS cache lock poisoned".to_owned()))?;
        cache.keys = Some(keys);
        cache.fetched_at = Some(Instant::now());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workos_verifier_new_targets_the_client_jwks() {
        let verifier = WorkosVerifier::new("client_abc123");
        // The JWKS URL is private, but the derived Debug output carries it.
        assert!(
            format!("{verifier:?}").contains("https://api.workos.com/sso/jwks/client_abc123"),
            "verifier should point at the per-client JWKS endpoint"
        );
    }

    #[test]
    fn workos_verifier_rejects_unusable_tokens_before_any_network() {
        let verifier = WorkosVerifier::new("client_abc123");
        // Not a JWT at all -> rejected at header decode, no JWKS fetch.
        assert!(verifier.verify("not-a-jwt").is_err());

        // Well-formed header with no `kid` -> rejected before any key lookup.
        use base64::Engine;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = b64.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = b64.encode(br#"{"sub":"user_1"}"#);
        let token = format!("{header}.{payload}.signature");
        let err = verifier
            .verify(&token)
            .expect_err("a kid-less token is rejected");
        assert!(matches!(err, AuthError::AuthRejected(_)));
    }
}
