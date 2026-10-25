use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use super::{Sealed, SecretName};

const KEY_BYTES: usize = 32;

/// The key Secrets are encrypted under: `BIA_MASTER_KEY`, 64 hex digits (ADR-0049).
#[derive(Clone)]
pub struct MasterKey([u8; KEY_BYTES]);

#[derive(Debug, thiserror::Error)]
#[error("the master key must be 64 hex digits (32 bytes), e.g. from `openssl rand -hex 32`")]
pub struct MasterKeyError;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("cannot draw random bytes for a nonce: {0}")]
    Random(getrandom::Error),
    #[error("encryption failed")]
    Seal,
    /// Wrong master key, or the stored value was changed.
    #[error("the stored value does not decrypt under the master key")]
    Open,
}

impl MasterKey {
    pub fn parse(hex: &str) -> Result<Self, MasterKeyError> {
        let mut key = [0; KEY_BYTES];
        hex::decode_to_slice(hex.trim(), &mut key).map_err(|_| MasterKeyError)?;
        Ok(Self(key))
    }

    /// Encrypts `value` under a fresh random nonce, binding it to `name`.
    pub fn seal(&self, name: &SecretName, value: &str) -> Result<Sealed, CryptoError> {
        let mut nonce = [0; 24];
        getrandom::fill(&mut nonce).map_err(CryptoError::Random)?;
        let ciphertext = XChaCha20Poly1305::new((&self.0).into())
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: value.as_bytes(),
                    aad: name.as_str().as_bytes(),
                },
            )
            .map_err(|_| CryptoError::Seal)?;
        Ok(Sealed { nonce, ciphertext })
    }

    pub fn open(&self, name: &SecretName, sealed: &Sealed) -> Result<String, CryptoError> {
        let plain = XChaCha20Poly1305::new((&self.0).into())
            .decrypt(
                &XNonce::from(sealed.nonce),
                Payload {
                    msg: &sealed.ciphertext,
                    aad: name.as_str().as_bytes(),
                },
            )
            .map_err(|_| CryptoError::Open)?;
        String::from_utf8(plain).map_err(|_| CryptoError::Open)
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MasterKey(..)")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn key(digit: char) -> MasterKey {
        MasterKey::parse(&digit.to_string().repeat(64)).unwrap()
    }

    #[test]
    fn a_value_round_trips_and_the_ciphertext_hides_it() {
        let name = SecretName::new("API_TOKEN").unwrap();
        let sealed = key('a').seal(&name, "s3cr3t-value").unwrap();
        assert!(!String::from_utf8_lossy(&sealed.ciphertext).contains("s3cr3t"));
        assert_eq!(key('a').open(&name, &sealed).unwrap(), "s3cr3t-value");
    }

    #[test]
    fn another_key_or_another_name_does_not_open_it() {
        let name = SecretName::new("A").unwrap();
        let sealed = key('a').seal(&name, "v").unwrap();
        assert!(key('b').open(&name, &sealed).is_err());
        assert!(
            key('a')
                .open(&SecretName::new("B").unwrap(), &sealed)
                .is_err()
        );
    }

    #[test]
    fn keys_that_are_not_64_hex_digits_are_refused() {
        for bad in ["", "abc", &"g".repeat(64), &"a".repeat(62), &"a".repeat(66)] {
            assert!(MasterKey::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn names_follow_the_expression_identifier_rules() {
        for ok in ["A", "API_TOKEN", "_x", "a1"] {
            assert!(SecretName::new(ok).is_ok(), "{ok}");
        }
        for bad in ["", "1a", "a-b", "a b", "é", &"a".repeat(129)] {
            assert!(SecretName::new(bad).is_err(), "{bad}");
        }
    }
}
