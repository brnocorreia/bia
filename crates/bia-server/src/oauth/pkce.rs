//! PKCE (RFC 7636). Only `S256` is accepted: `plain` protects nothing a code thief lacks.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// A code verifier is 43 to 128 characters of `A-Z a-z 0-9 - . _ ~`; a challenge, being the
/// base64url of a SHA-256, is exactly 43 characters of the base64url alphabet.
pub(crate) fn is_challenge(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub(crate) fn is_verifier(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

/// Whether `verifier` is the one `challenge` was made from.
pub(crate) fn verifies(verifier: &str, challenge: &str) -> bool {
    let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    computed.as_bytes().ct_eq(challenge.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::{is_challenge, is_verifier, verifies};

    // The example of RFC 7636 appendix B.
    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    #[test]
    fn the_rfc_example_verifies_and_nothing_else_does() {
        assert!(verifies(VERIFIER, CHALLENGE));
        assert!(!verifies("a".repeat(43).as_str(), CHALLENGE));
        assert!(!verifies(VERIFIER, VERIFIER));
    }

    #[test]
    fn challenges_and_verifiers_have_their_shapes() {
        assert!(is_challenge(CHALLENGE));
        assert!(!is_challenge("short"));
        assert!(!is_challenge(&format!("{CHALLENGE}=")));
        assert!(is_verifier(VERIFIER));
        assert!(is_verifier(&"a".repeat(128)));
        assert!(!is_verifier(&"a".repeat(42)));
        assert!(!is_verifier(&"a".repeat(129)));
        assert!(!is_verifier(&format!("{}!", "a".repeat(43))));
    }
}
