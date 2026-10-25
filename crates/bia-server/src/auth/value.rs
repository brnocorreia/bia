//! Token values: how they are minted, recognised and hashed (ADR-0043).

use std::fmt;

use bia_store::token::TokenHash;
use sha2::{Digest, Sha256};

/// Every Token value starts with it, so secret scanners can recognise a leaked one.
const PREFIX: &str = "bia_";
/// 256 random bits, written as hex after [`PREFIX`].
const RANDOM_BYTES: usize = 32;

/// A Token's value, which the API shows once and never stores.
pub(crate) struct TokenValue(String);

impl TokenValue {
    /// A new random value and the hash the store keeps of it.
    pub(crate) fn generate() -> Result<(Self, TokenHash), getrandom::Error> {
        let mut random = [0; RANDOM_BYTES];
        getrandom::fill(&mut random)?;
        let value = format!("{PREFIX}{}", hex::encode(random));
        let hash = hash(&value);
        Ok((Self(value), hash))
    }

    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Debug for TokenValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenValue(..)")
    }
}

/// Whether `value` has the shape of a Token value, so a stray credential of another kind is
/// rejected without a database lookup.
pub(crate) fn is_token_value(value: &str) -> bool {
    value.strip_prefix(PREFIX).is_some_and(|random| {
        random.len() == RANDOM_BYTES * 2
            && random
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// SHA-256, unkeyed: a value carries 256 random bits, so a stolen hash cannot be reversed by
/// guessing, and lookups need no key the server would have to keep (ADR-0043).
pub(crate) fn hash(value: &str) -> TokenHash {
    TokenHash(Sha256::digest(value.as_bytes()).into())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{TokenValue, hash, is_token_value};

    #[test]
    fn a_generated_value_is_recognised_and_hashes_to_what_the_store_keeps() {
        let (value, stored) = TokenValue::generate().unwrap();
        let value = value.into_string();

        assert!(is_token_value(&value), "{value}");
        assert_eq!(value.len(), 68);
        assert!(hash(&value) == stored);
        assert!(!is_token_value(&value.to_uppercase()));
        assert!(!is_token_value(&value[..67]));
        assert!(!is_token_value("ghp_0123456789abcdef"));
    }

    #[test]
    fn two_generated_values_differ() {
        let (first, _) = TokenValue::generate().unwrap();
        let (second, _) = TokenValue::generate().unwrap();

        assert_ne!(first.into_string(), second.into_string());
    }
}
