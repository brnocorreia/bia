//! What the UI's consent screen calls. A person decides an authorization the server recorded:
//! the client, redirect URI, resource and the Scope it asked for are the server's, so the
//! screen can only choose to approve (with that Scope or less) or deny.
//!
//! These calls need a Session: an `admin` Token of another origin, such as an OAuth Grant's,
//! must not be able to approve more access for itself. With the cookie they pass the `Origin`
//! check like every unsafe request.

use axum::extract::State;
use axum::http::StatusCode;
use bia_store::oauth::{
    Authorization, GrantScope, NewApproval, OAuthRepository, PgOAuthRepository, Provenance,
};
use bia_store::token::{Origin, Token};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::authorize::Answer;
use crate::auth::{Admin, AuthErrors, TokenValue};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path};

/// Which scope a person can grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GrantScopeName {
    /// Author Drafts, validate, dry-run, trigger and read; never publish or activate.
    Agent,
    /// Everything.
    Admin,
}

impl From<GrantScope> for GrantScopeName {
    fn from(scope: GrantScope) -> Self {
        match scope {
            GrantScope::Admin => Self::Admin,
            GrantScope::Agent => Self::Agent,
        }
    }
}

impl From<GrantScopeName> for GrantScope {
    fn from(scope: GrantScopeName) -> Self {
        match scope {
            GrantScopeName::Admin => Self::Admin,
            GrantScopeName::Agent => Self::Agent,
        }
    }
}

/// Where a client's identity comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProvenanceName {
    /// A Client ID Metadata Document served from `host`.
    Cimd,
    /// The client registered itself; nothing vouches for its name.
    Dynamic,
    /// Shipped with bia.
    Builtin,
}

impl From<Provenance> for ProvenanceName {
    fn from(provenance: Provenance) -> Self {
        match provenance {
            Provenance::Cimd => Self::Cimd,
            Provenance::Dynamic => Self::Dynamic,
            Provenance::Builtin => Self::Builtin,
        }
    }
}

/// The client asking for access.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ConsentClient {
    pub(super) id: String,
    /// The name the client gave itself.
    pub(super) name: String,
    pub(super) provenance: ProvenanceName,
    /// For `cimd`: the host the client's metadata was fetched from.
    pub(super) host: Option<String>,
    /// `false` for a client that registered itself: its name is unchecked.
    pub(super) verified: bool,
}

/// What a person is asked to approve.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ConsentRequest {
    id: Uuid,
    client: ConsentClient,
    /// Where the person is sent back to, with the result.
    redirect_uri: String,
    /// What the client asked for; the person may approve less.
    requested_scope: GrantScopeName,
    /// What the tokens will be good for: the API, or the MCP endpoint only.
    resource: String,
    expires_at: DateTime<Utc>,
}

impl From<Authorization> for ConsentRequest {
    fn from(authorization: Authorization) -> Self {
        Self {
            id: authorization.id,
            client: ConsentClient {
                id: authorization.client_id,
                name: authorization.client_name,
                provenance: authorization.provenance.into(),
                host: authorization.client_host,
                verified: authorization.provenance != Provenance::Dynamic,
            },
            redirect_uri: authorization.redirect_uri,
            requested_scope: authorization.requested_scope.into(),
            resource: authorization.resource,
            expires_at: authorization.expires_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Approval {
    /// The Scope to grant: what was requested, or `agent` to downgrade `admin`. Asking for more
    /// than was requested is refused. Defaults to what was requested.
    #[serde(default)]
    pub(super) scope: Option<GrantScopeName>,
}

/// Where to send the person: the client's redirect URI with the result.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Decision {
    redirect_to: String,
}

pub(super) fn require_session(caller: &Token) -> Result<(), ApiError> {
    if caller.origin == Origin::Session {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        ErrorCode::NotASession,
        "only a person's Session can answer an authorization request",
    ))
}

fn not_found() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        ErrorCode::AuthorizationNotFound,
        "no pending authorization has this id: it was decided already, or it ran out; start \
         again from the client",
    )
}

/// The authorization request a person is asked about. Needs a Session.
#[utoipa::path(
    get,
    path = "/oauth/consent/{request_id}",
    tag = "oauth",
    security(("session" = []), ("token" = [])),
    params(("request_id" = Uuid, Path, description = "The `request` in the consent screen's URL.")),
    responses(
        (status = OK, body = ConsentRequest),
        (status = NOT_FOUND, description = "`authorization_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn get_consent(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Path(request_id): Path<Uuid>,
) -> Result<Json<ConsentRequest>, ApiError> {
    require_session(&caller)?;
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let pending = PgOAuthRepository
        .pending_authorization(&mut conn, request_id, state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(not_found)?;
    Ok(Json(pending.into()))
}

/// Approves the request with the Scope given or less, and returns where to send the person:
/// the client's `redirect_uri` with a single-use `code` (valid for a minute), `state` and
/// `iss`. Answering twice fails. Needs a Session.
#[utoipa::path(
    post,
    path = "/oauth/consent/{request_id}/approve",
    tag = "oauth",
    security(("session" = []), ("token" = [])),
    params(("request_id" = Uuid, Path, description = "The `request` in the consent screen's URL.")),
    request_body = Approval,
    responses(
        (status = OK, body = Decision),
        (status = BAD_REQUEST, description = "`scope_exceeds_request`.", body = ErrorBody),
        (status = NOT_FOUND, description = "`authorization_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn approve(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Path(request_id): Path<Uuid>,
    Json(approval): Json<Approval>,
) -> Result<Json<Decision>, ApiError> {
    require_session(&caller)?;
    let now = state.clock.now();
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let pending = PgOAuthRepository
        .pending_authorization(&mut conn, request_id, now)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(not_found)?;
    let scope = approval
        .scope
        .map_or(pending.requested_scope, GrantScope::from);
    if !pending.requested_scope.covers(scope) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::ScopeExceedsRequest,
            "the client asked for less than that; approve what it asked for, or less",
        ));
    }
    let (code, hash) = TokenValue::generate_prefixed(super::AUTHORIZATION_CODE_PREFIX)
        .map_err(|e| ApiError::internal(&e))?;
    let approved = PgOAuthRepository
        .approve(
            &mut conn,
            request_id,
            &NewApproval {
                user_id: caller.user_id,
                scope,
                code: hash,
                now,
                code_expires_at: now + super::CODE_LIFETIME,
            },
        )
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(not_found)?;
    tracing::info!(
        authorization_id = %approved.id,
        client = %approved.client_name,
        scope = scope.as_str(),
        "authorization approved"
    );
    let issuer = state.config.issuer()?;
    let redirect_to = answer(&approved, &issuer)
        .url(&[("code", code.as_str())])
        .ok_or_else(|| ApiError::internal(&std::fmt::Error))?;
    Ok(Json(Decision { redirect_to }))
}

/// Denies the request and returns where to send the person: the client's `redirect_uri` with
/// `error=access_denied`, `state` and `iss`. Needs a Session.
#[utoipa::path(
    post,
    path = "/oauth/consent/{request_id}/deny",
    tag = "oauth",
    security(("session" = []), ("token" = [])),
    params(("request_id" = Uuid, Path, description = "The `request` in the consent screen's URL.")),
    responses(
        (status = OK, body = Decision),
        (status = NOT_FOUND, description = "`authorization_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn deny(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Path(request_id): Path<Uuid>,
) -> Result<Json<Decision>, ApiError> {
    require_session(&caller)?;
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let denied = PgOAuthRepository
        .deny(&mut conn, request_id, state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(not_found)?;
    tracing::info!(authorization_id = %denied.id, client = %denied.client_name, "authorization denied");
    let issuer = state.config.issuer()?;
    let redirect_to = answer(&denied, &issuer)
        .url(&[
            ("error", "access_denied"),
            ("error_description", "the user denied the request"),
        ])
        .ok_or_else(|| ApiError::internal(&std::fmt::Error))?;
    Ok(Json(Decision { redirect_to }))
}

fn answer<'a>(authorization: &'a Authorization, issuer: &'a str) -> Answer<'a> {
    Answer {
        redirect_uri: &authorization.redirect_uri,
        state: authorization.state.as_deref(),
        issuer,
    }
}
