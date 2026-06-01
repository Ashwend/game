//! PKCE (RFC 7636) and URL-encoding helpers for the native WorkOS login.
//!
//! Public clients can't keep a secret, so PKCE proves the app that started the
//! authorize request is the same one redeeming the code: a random verifier is
//! hashed into the `code_challenge` sent up front, and the raw verifier is
//! presented when swapping the code for tokens.

use base64::Engine;
use sha2::{Digest, Sha256};

/// Random base64url token of `bytes` bytes of OS entropy (via UUID v4), used
/// for the PKCE verifier and the CSRF `state`.
pub(super) fn random_token(bytes: usize) -> String {
    let mut raw = Vec::with_capacity(bytes);
    while raw.len() < bytes {
        raw.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    raw.truncate(bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

/// The `S256` PKCE challenge for a verifier: base64url(SHA-256(verifier)).
pub(super) fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

pub(super) fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub(super) fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_challenge_is_url_safe_base64_of_sha256() {
        // Known RFC 7636 appendix-B vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            code_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn percent_round_trips() {
        let raw = "http://127.0.0.1:8765/callback?x=a b&y=z";
        assert_eq!(percent_decode(&percent_encode(raw)), raw);
    }

    #[test]
    fn random_token_has_expected_length_and_charset() {
        let token = random_token(64);
        // base64url-no-pad of 64 bytes is 86 chars, all URL-safe.
        assert_eq!(token.len(), 86);
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }

    #[test]
    fn percent_decode_handles_plus_and_malformed_escapes() {
        // `+` decodes to a space (form encoding).
        assert_eq!(percent_decode("a+b"), "a b");
        // A valid escape decodes; a malformed one is passed through verbatim.
        assert_eq!(percent_decode("%41"), "A");
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
        // A trailing, truncated escape can't be decoded and is left as-is.
        assert_eq!(percent_decode("x%4"), "x%4");
    }
}
