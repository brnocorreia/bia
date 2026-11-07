//! Issuing what a Grant is for: an access token and a refresh token.

use bia_store::oauth::{Grant, GrantScope, OAuthRepository, PgOAuthRepository};
use bia_store::token::{NewToken, Origin, PgTokenRepository, TokenRepository};
use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;

use super::{ACCESS_TOKEN_LIFETIME, REFRESH_TOKEN_LIFETIME, REFRESH_TOKEN_PREFIX};
use crate::auth::TokenValue;
use crate::{ApiError, ApiState};

/// A token response (RFC 6749 section 5.1).
// No `Debug`, so the values cannot reach a log through one.
#[derive(Serialize, ToSchema)]
pub(crate) struct IssuedTokens {
    /// An `oauth` Token for the Grant's `resource`, valid for `expires_in` seconds.
    access_token: String,
    token_type: &'static str,
    /// Seconds: 3600.
    expires_in: i64,
    /// Single-use: exchanging it returns a new one. Presenting an old one again revokes the
    /// Grant. Unused for 90 days it stops working.
    refresh_token: String,
    /// The Scope granted: `agent` or `admin`.
    scope: &'static str,
}

/// Mints an access token of `scope` (at most the Grant's) and a new refresh token for `grant`,
/// on `conn`, so a caller can commit them with whatever else must happen together.
pub(crate) async fn issue_tokens(
    state: &ApiState,
    conn: &mut PgConnection,
    grant: &Grant,
    scope: GrantScope,
) -> Result<IssuedTokens, ApiError> {
    let now = state.clock.now();
    let (access, hash) = TokenValue::generate().map_err(|e| ApiError::internal(&e))?;
    PgTokenRepository
        .create(
            conn,
            &NewToken {
                name: grant.client_name.clone(),
                hash,
                scope: scope.scope(),
                origin: Origin::OAuth,
                user_id: grant.user_id,
                created_at: now,
                user_agent: None,
                grant_id: Some(grant.id),
                expires_at: Some(now + ACCESS_TOKEN_LIFETIME),
                audience: Some(grant.resource.clone()),
            },
        )
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let (refresh, refresh_hash) =
        TokenValue::generate_prefixed(REFRESH_TOKEN_PREFIX).map_err(|e| ApiError::internal(&e))?;
    PgOAuthRepository
        .add_refresh_token(
            conn,
            grant.id,
            refresh_hash,
            now,
            now + REFRESH_TOKEN_LIFETIME,
        )
        .await
        .map_err(|e| ApiError::internal(&e))?;
    Ok(IssuedTokens {
        access_token: access.into_string(),
        token_type: "Bearer",
        expires_in: ACCESS_TOKEN_LIFETIME.num_seconds(),
        refresh_token: refresh.into_string(),
        scope: scope.as_str(),
    })
}
