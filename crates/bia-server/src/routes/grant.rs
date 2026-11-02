//! Grants: the OAuth clients a person approved, listed and revoked from the Access screen.

use axum::extract::State;
use axum::http::StatusCode;
use bia_store::oauth::{Grant, OAuthRepository, PgOAuthRepository};
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{Admin, AuthErrors};
use crate::oauth::consent::{GrantScopeName, ProvenanceName};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path};

/// What a person approved for an OAuth client.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct GrantInfo {
    id: Uuid,
    client_id: String,
    client_name: String,
    provenance: ProvenanceName,
    /// For `cimd`: the host the client's metadata came from.
    client_host: Option<String>,
    scope: GrantScopeName,
    /// The audience of its tokens: the API's `public_url`, or `{public_url}/mcp`.
    resource: String,
    created_at: DateTime<Utc>,
    /// When one of its access tokens last authenticated a request, to the minute.
    last_used_at: Option<DateTime<Utc>>,
    /// Once set, none of its tokens authenticates anything.
    revoked_at: Option<DateTime<Utc>>,
}

impl From<Grant> for GrantInfo {
    fn from(grant: Grant) -> Self {
        Self {
            id: grant.id,
            client_id: grant.client_id,
            client_name: grant.client_name,
            provenance: grant.provenance.into(),
            client_host: grant.client_host,
            scope: grant.scope.into(),
            resource: grant.resource,
            created_at: grant.created_at,
            last_used_at: grant.last_used_at,
            revoked_at: grant.revoked_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct GrantList {
    /// Every Grant, revoked ones included, oldest first.
    grants: Vec<GrantInfo>,
}

/// Every Grant, revoked ones included. Needs an `admin` Token.
#[utoipa::path(
    get,
    path = "/v1/grants",
    tag = "grants",
    security(("token" = []), ("session" = [])),
    responses((status = OK, body = GrantList), AuthErrors)
)]
pub(crate) async fn list_grants(
    _: Admin,
    State(state): State<ApiState>,
) -> Result<Json<GrantList>, ApiError> {
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let grants = PgOAuthRepository
        .grants(&mut conn)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    Ok(Json(GrantList {
        grants: grants.into_iter().map(GrantInfo::from).collect(),
    }))
}

/// Revokes the Grant at once, with every access token and refresh token it issued; revoking it
/// again changes nothing. Needs an `admin` Token.
#[utoipa::path(
    post,
    path = "/v1/grants/{grant_id}/revoke",
    tag = "grants",
    security(("token" = []), ("session" = [])),
    params(("grant_id" = Uuid, Path, description = "The Grant's `id`.")),
    responses(
        (status = OK, body = GrantInfo),
        (status = NOT_FOUND, description = "`grant_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn revoke_grant(
    _: Admin,
    State(state): State<ApiState>,
    Path(grant_id): Path<Uuid>,
) -> Result<Json<GrantInfo>, ApiError> {
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let grant = PgOAuthRepository
        .revoke_grant(&mut conn, grant_id, state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                ErrorCode::GrantNotFound,
                format!("no Grant has id {grant_id}"),
            )
        })?;
    tracing::info!(grant_id = %grant.id, client = %grant.client_name, "grant revoked");
    Ok(Json(grant.into()))
}
