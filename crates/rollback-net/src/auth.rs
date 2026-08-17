//! Datagram authentication.
//!
//! Every datagram carries an HMAC-SHA256 tag over its entire body. UDP has no
//! connection state, so without this anyone who can guess the port can inject a
//! frame of input into somebody's match -- or, worse, an `InputBatch` that
//! contradicts a confirmed frame and kills the session.
//!
//! What this does *not* do: it is not encryption (inputs are not secret) and it
//! is not a replay defence on its own (the session's own frame accounting makes
//! a replayed batch a no-op). It answers exactly one question: did the peer
//! holding the session key send this?
//!
//! The key is ephemeral, generated per session, and never enters Terraform
//! state -- it is written to SSM Parameter Store as a SecureString and read by
//! the instance at boot. See `docs/06-aws.md`.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::wire::TAG_LEN;

type HmacSha256 = Hmac<Sha256>;

/// Length of a session key, in bytes.
pub const KEY_LEN: usize = 32;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("datagram is shorter than the authentication tag")]
    TooShort,
    #[error("authentication tag does not verify")]
    BadTag,
    #[error("session key must be {KEY_LEN} bytes, got {0}")]
    BadKeyLength(usize),
    #[error("session key is not valid hex")]
    BadKeyHex,
}

/// Holds the session key and signs/verifies datagrams with it.
#[derive(Clone)]
pub struct Authenticator {
    key: [u8; KEY_LEN],
}

impl std::fmt::Debug for Authenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never let the key reach a log line by accident.
        f.write_str("Authenticator(<redacted>)")
    }
}

impl Authenticator {
    pub fn new(key: [u8; KEY_LEN]) -> Self {
        Authenticator { key }
    }

    /// Parse a 64-character hex key, the form used in SSM and the environment.
    pub fn from_hex(hex: &str) -> Result<Self, AuthError> {
        let hex = hex.trim();
        if hex.len() != KEY_LEN * 2 {
            return Err(AuthError::BadKeyLength(hex.len() / 2));
        }
        let mut key = [0u8; KEY_LEN];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| AuthError::BadKeyHex)?;
        }
        Ok(Authenticator { key })
    }

    /// Derive a key deterministically from a passphrase.
    ///
    /// Only for local development and tests: it is a single SHA-256 pass, not a
    /// password KDF. `just aws-up` generates a random key instead.
    pub fn from_passphrase(passphrase: &str) -> Self {
        use sha2::Digest;
        let mut hasher = Sha256::new();
        hasher.update(b"rollback-netcode/session-key/v1");
        hasher.update(passphrase.as_bytes());
        let digest = hasher.finalize();
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&digest);
        Authenticator { key }
    }

    fn tag(&self, body: &[u8]) -> [u8; TAG_LEN] {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("HMAC accepts any key length");
        mac.update(body);
        let out = mac.finalize().into_bytes();
        let mut tag = [0u8; TAG_LEN];
        tag.copy_from_slice(&out);
        tag
    }

    /// Append the authentication tag, producing the datagram to put on the wire.
    pub fn seal(&self, mut body: Vec<u8>) -> Vec<u8> {
        let tag = self.tag(&body);
        body.extend_from_slice(&tag);
        body
    }

    /// Verify and strip the tag, returning the message body.
    ///
    /// Verification uses the `hmac` crate's constant-time comparison: a
    /// byte-by-byte early return would leak the tag one byte at a time to an
    /// attacker who can time the responses.
    pub fn open<'a>(&self, datagram: &'a [u8]) -> Result<&'a [u8], AuthError> {
        if datagram.len() <= TAG_LEN {
            return Err(AuthError::TooShort);
        }
        let split = datagram.len() - TAG_LEN;
        let (body, tag) = datagram.split_at(split);

        let mut mac = HmacSha256::new_from_slice(&self.key).expect("HMAC accepts any key length");
        mac.update(body);
        mac.verify_slice(tag).map_err(|_| AuthError::BadTag)?;
        Ok(body)
    }
}

/// Render a key as lowercase hex, for writing it into SSM.
pub fn key_to_hex(key: &[u8; KEY_LEN]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Authenticator {
        Authenticator::from_passphrase("lab")
    }

    #[test]
    fn a_sealed_datagram_opens_to_the_original_body() {
        let body = b"the quick brown fox".to_vec();
        let sealed = auth().seal(body.clone());
        assert_eq!(sealed.len(), body.len() + TAG_LEN);
        assert_eq!(auth().open(&sealed).unwrap(), &body[..]);
    }

    #[test]
    fn a_flipped_body_bit_fails_verification() {
        let mut sealed = auth().seal(b"frame 42 input 0x0010".to_vec());
        sealed[3] ^= 0x01;
        assert_eq!(auth().open(&sealed), Err(AuthError::BadTag));
    }

    #[test]
    fn a_flipped_tag_bit_fails_verification() {
        let mut sealed = auth().seal(b"frame 42".to_vec());
        let last = sealed.len() - 1;
        sealed[last] ^= 0x80;
        assert_eq!(auth().open(&sealed), Err(AuthError::BadTag));
    }

    #[test]
    fn a_different_key_cannot_open_the_datagram() {
        let sealed = auth().seal(b"hello".to_vec());
        let stranger = Authenticator::from_passphrase("not the lab key");
        assert_eq!(stranger.open(&sealed), Err(AuthError::BadTag));
    }

    #[test]
    fn a_datagram_with_no_room_for_a_tag_is_rejected() {
        assert_eq!(auth().open(&[0u8; TAG_LEN]), Err(AuthError::TooShort));
        assert_eq!(auth().open(&[]), Err(AuthError::TooShort));
    }

    #[test]
    fn hex_keys_round_trip() {
        let key = [0x5Au8; KEY_LEN];
        let hex = key_to_hex(&key);
        assert_eq!(hex.len(), 64);
        let parsed = Authenticator::from_hex(&hex).unwrap();
        let sealed = parsed.seal(b"x".to_vec());
        assert!(Authenticator::new(key).open(&sealed).is_ok());
    }

    #[test]
    fn malformed_hex_keys_are_rejected() {
        assert_eq!(
            Authenticator::from_hex("abcd").unwrap_err(),
            AuthError::BadKeyLength(2)
        );
        let bad = "z".repeat(64);
        assert_eq!(Authenticator::from_hex(&bad).unwrap_err(), AuthError::BadKeyHex);
    }

    #[test]
    fn the_key_never_appears_in_debug_output() {
        let rendered = format!("{:?}", Authenticator::new([0xAB; KEY_LEN]));
        assert!(!rendered.contains("ab"), "debug leaked key bytes: {rendered}");
        assert!(rendered.contains("redacted"));
    }
}
