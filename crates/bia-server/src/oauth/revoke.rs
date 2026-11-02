//! `POST /oauth/revoke` (RFC 7009): a client gives up its access.

use axum::extract::State;
use axum::extract::rejection::FormRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Form, Json};
use bia_store::oauth::{OAuthRepository, PgOAuthRepository};
use serde::Deserialize;
use utoipa::ToSchema;

use super::error::no_store;
use super::{OAuthError, OAuthErrorBody};
use crate::ApiState;
use crate::auth::secret_hash;

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct RevokeRequest {
    /// An access token or a refresh token the client was issued.
    token: Option<String>,
    /// The client's id; a token is only revoked for the client it was issued to.
    client_id: Option<String>,
    /// Ignored: either kind is recognised.
    #[allow(dead_code)]
    token_type_hint: Option<String>,
}

/// Revokes the Grant the token belongs to, and with it every access token and refresh token it
/// issued: a client that is done wants all of it gone. Answers 200 whether or not the token
/// was known. Needs no credential, since holding the token is the proof.
#[utoipa::path(
    post,
    path = "/oauth/revoke",
    tag = "oauth",
    request_body(content = RevokeRequest, content_type = "application/x-www-form-urlencoded"),
    responses(
        (status = OK, description = "Revoked, or unknown."),
        (status = BAD_REQUEST, description = "`invalid_request`.", body = OAuthErrorBody),
    )
)]
pub(crate) async fn revoke(
    State(state): State<ApiState>,
    request: Result<Form<RevokeRequest>, FormRejection>,
) -> Result<Response, OAuthError> {
    let Form(request) = request.map_err(|_| {
        OAuthError::invalid_request("the body must be `application/x-www-form-urlencoded`")
    })?;
    let (Some(token), Some(client_id)) = (
        request.token.as_deref().filter(|t| !t.is_empty()),
        request.client_id.as_deref().filter(|c| !c.is_empty()),
    ) else {
        return Err(OAuthError::invalid_request(
            "`token` and `client_id` are required",
        ));
    };
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    let hash = secret_hash(token);
    let grant = PgOAuthRepository
        .grant_of_secret(&mut conn, hash)
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    if let Some(grant) = grant.filter(|grant| grant.client_id == client_id) {
        PgOAuthRepository
            .revoke_grant(&mut conn, grant.id, state.clock.now())
            .await
            .map_err(|e| crate::ApiError::internal(&e))?;
        tracing::info!(grant_id = %grant.id, "grant revoked by its client");
    }
    let mut response = (StatusCode::OK, Json(serde_json::json!({}))).into_response();
    no_store(&mut response);
    Ok(response)
}
