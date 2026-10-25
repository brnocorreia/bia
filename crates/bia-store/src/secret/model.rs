use std::fmt;

use chrono::{DateTime, Utc};

const MAX_NAME_LEN: usize = 128;

/// A Secret's name, as `secrets.NAME` reads it: ASCII letters, digits and `_`, not starting
/// with a digit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a Secret name is 1 to 128 ASCII letters, digits or `_`, and does not start with a digit")]
pub struct InvalidSecretName;

impl SecretName {
    pub fn new(name: impl Into<String>) -> Result<Self, InvalidSecretName> {
        let name = name.into();
        let valid = !name.is_empty()
            && name.len() <= MAX_NAME_LEN
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if valid {
            Ok(Self(name))
        } else {
            Err(InvalidSecretName)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A Secret without its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretInfo {
    pub name: SecretName,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// An encrypted value with the nonce it was encrypted under.
#[derive(Clone, PartialEq, Eq)]
pub struct Sealed {
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

impl fmt::Debug for Sealed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sealed(..)")
    }
}
