//! At-rest obfuscation for local client files: the settings file and the
//! WorkOS refresh-token store.
//!
//! This is deliberately **not** a security boundary. The key is baked into the
//! binary, so anyone with the source (this project is open) can decrypt a
//! sealed file. The goal is narrower: keep the on-disk bytes out of plain text
//! so a casual file or process scan can't lift a session token or read the
//! config as a string.
//!
//! We use a real AEAD (ChaCha20-Poly1305) rather than a hand-rolled XOR so a
//! truncated, tampered, or foreign blob fails cleanly to "couldn't decrypt"
//! (an authentication-tag mismatch) instead of silently decoding to garbage.
//! Callers turn that failure into "reset to defaults" / "no stored session",
//! which is exactly what we want for an old plain-text file written by a build
//! that predates this module.

use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit},
};
use uuid::Uuid;

/// Static obfuscation key. See the module docs: this is not a secret. Changing
/// it simply invalidates every previously sealed file (they reset to defaults /
/// log out on next launch).
const LOCAL_KEY: [u8; 32] = [
    0x3b, 0x7d, 0x1c, 0xa8, 0x52, 0x9f, 0x46, 0xe1, 0x0d, 0xb4, 0x77, 0x29, 0x8e, 0xc3, 0x61, 0x5a,
    0xf2, 0x14, 0x9b, 0x6c, 0xd0, 0x35, 0xa7, 0x88, 0x4e, 0xbd, 0x12, 0x70, 0xe9, 0x53, 0x2f, 0xc6,
];

/// Magic prefix so a non-sealed blob (an old plain-text file, or anything else)
/// is rejected up front instead of being fed to the cipher as if it were ours.
/// The trailing digits are a format version; bump them on any framing change.
const MAGIC: &[u8; 8] = b"ASHWND01";

/// ChaCha20-Poly1305 nonce length.
const NONCE_LEN: usize = 12;

/// Seal `plaintext` into `MAGIC || nonce || ciphertext+tag`.
pub fn seal(plaintext: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&LOCAL_KEY));
    // A fresh random nonce per write: ChaCha20-Poly1305 needs a unique nonce
    // per (key, message). A v4 UUID gives 16 random bytes; the first 12 are the
    // nonce, so we don't pull a separate RNG dependency.
    let uuid = Uuid::new_v4();
    let nonce_bytes = &uuid.as_bytes()[..NONCE_LEN];
    let nonce = Nonce::from_slice(nonce_bytes);
    // Encryption only errors on absurd input sizes we never produce; treat the
    // theoretical failure as an empty body so the file stays well-formed and
    // simply opens back to nothing (caller resets).
    let ciphertext = cipher.encrypt(nonce, plaintext).unwrap_or_default();

    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(nonce_bytes);
    out.extend_from_slice(&ciphertext);
    out
}

/// Open a blob produced by [`seal`]. Returns `None` for anything that isn't a
/// well-formed, authentic sealed blob (wrong magic, truncated, tampered, or an
/// old plain-text file), so callers can cleanly fall back to defaults.
pub fn open(data: &[u8]) -> Option<Vec<u8>> {
    let rest = data.strip_prefix(&MAGIC[..])?;
    if rest.len() < NONCE_LEN {
        return None;
    }
    let (nonce_bytes, ciphertext) = rest.split_at(NONCE_LEN);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&LOCAL_KEY));
    cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trips() {
        let plaintext = br#"{"hello":"world","n":42}"#;
        let sealed = seal(plaintext);
        // The payload must not appear verbatim in the sealed bytes.
        assert!(
            sealed.windows(plaintext.len()).all(|w| w != plaintext),
            "sealed blob should not contain the plaintext"
        );
        assert_eq!(open(&sealed).as_deref(), Some(&plaintext[..]));
    }

    #[test]
    fn two_seals_differ_but_both_open() {
        // Random per-write nonce means identical input seals to different bytes.
        let plaintext = b"same input";
        let a = seal(plaintext);
        let b = seal(plaintext);
        assert_ne!(a, b, "fresh nonce should change the sealed bytes");
        assert_eq!(open(&a).as_deref(), Some(&plaintext[..]));
        assert_eq!(open(&b).as_deref(), Some(&plaintext[..]));
    }

    #[test]
    fn open_rejects_plain_text_and_garbage() {
        // An old plain-text settings file (no magic) must not open.
        assert!(open(br#"{"display":{}}"#).is_none());
        // Right magic, but a truncated / tampered body fails the auth tag.
        let mut sealed = seal(b"payload");
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        assert!(open(&sealed).is_none());
        // Magic only, no nonce.
        assert!(open(MAGIC).is_none());
        // Empty input.
        assert!(open(&[]).is_none());
    }
}
