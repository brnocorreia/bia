//! Hashing and checking passwords with argon2id.

use argon2::password_hash::PasswordVerifier;
use argon2::password_hash::phc::PasswordHash as Phc;
use argon2::{Argon2, PasswordHasher};
use bia_store::user::PasswordHash;

use axum::http::StatusCode;

use crate::{ApiError, ErrorCode};

/// Counted in characters. No other rule: length is what makes a password hard to guess.
pub(crate) const MIN_LEN: usize = 12;

pub(crate) fn too_short() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::PasswordTooShort,
        format!("the password must be at least {MIN_LEN} characters"),
    )
}

/// Wrong password, or none to compare with: the same answer, so it says nothing about which.
pub(crate) fn invalid_credentials() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::InvalidCredentials,
        "the password is wrong",
    )
}

/// argon2id with the crate's defaults, which follow OWASP's recommendation, off the async
/// runtime since it is slow on purpose.
pub(crate) async fn hash(password: String) -> Result<PasswordHash, ApiError> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| PasswordHash(hash.to_string()))
    })
    .await
    .map_err(|e| ApiError::internal(&e))?
    .map_err(|e| ApiError::internal(&e))
}

/// Whether `password` is the one `stored` hashes. A stored hash that cannot be read counts as
/// a mismatch, and is logged.
pub(crate) async fn verify(password: String, stored: PasswordHash) -> Result<bool, ApiError> {
    tokio::task::spawn_blocking(move || {
        let Ok(phc) = stored.0.parse::<Phc>() else {
            tracing::error!("a stored password hash is not a PHC string");
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &phc)
            .is_ok()
    })
    .await
    .map_err(|e| ApiError::internal(&e))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{hash, verify};

    #[tokio::test]
    async fn only_the_hashed_password_verifies() {
        let stored = hash("correct horse battery".to_owned()).await.unwrap();

        assert!(
            verify("correct horse battery".to_owned(), stored.clone())
                .await
                .unwrap()
        );
        assert!(
            !verify("correct horse batterx".to_owned(), stored)
                .await
                .unwrap()
        );
    }
}
