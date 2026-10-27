//! Tokens: created, listed and revoked by `admin` (ADR-0043). A Token's value is in the response
//! that creates it and nowhere else.

use axum::extract::State;
use axum::http::StatusCode;
use bia_core::dsl::{TriggerId, WorkflowId};
use bia_store::token::{NewToken, PgTokenRepository, Scope, Token, TokenId, TokenRepository};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{Admin, Agent, AuthErrors, TokenValue};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path};

const MAX_NAME_LEN: usize = 128;

/// What a Token may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScopeName {
    /// Everything.
    Admin,
    /// Author Drafts, validate, dry-run, trigger and read; never publish or activate.
    Agent,
    /// Start Runs from one webhook Trigger, named in `webhook`.
    Webhook,
}

/// The webhook Trigger a `webhook` Token is bound to.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebhookBinding {
    #[schema(value_type = String, example = "onboarding-email")]
    workflow_id: WorkflowId,
    #[schema(value_type = String, example = "signup")]
    trigger_id: TriggerId,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewTokenRequest {
    /// What the Token is for, e.g. `claude-desktop`. Need not be unique.
    #[schema(example = "claude-desktop")]
    name: String,
    scope: ScopeName,
    /// Required with the `webhook` Scope, and refused with any other.
    #[serde(default)]
    webhook: Option<WebhookBinding>,
}

/// A Token, without its value.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct TokenInfo {
    id: Uuid,
    name: String,
    scope: ScopeName,
    /// Present with the `webhook` Scope.
    #[serde(skip_serializing_if = "Option::is_none")]
    webhook: Option<WebhookBinding>,
    created_at: DateTime<Utc>,
    /// When the Token last authenticated a request, to the minute.
    last_used_at: Option<DateTime<Utc>>,
    /// Once set, the Token authenticates nothing.
    revoked_at: Option<DateTime<Utc>>,
}

/// A Token just created, with its value.
// No `Debug`, so the value cannot reach a log through one.
#[derive(Serialize, ToSchema)]
pub(crate) struct CreatedToken {
    token: TokenInfo,
    /// Send it as `Authorization: Bearer <value>`. Shown only in this response: bia keeps a
    /// hash of it, so a lost value cannot be recovered, only replaced.
    #[schema(example = "bia_6f1c…")]
    value: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct TokenList {
    /// Every Token, revoked ones included, oldest first.
    tokens: Vec<TokenInfo>,
}

/// Creates a Token and returns its value, once. Needs an `admin` Token.
#[utoipa::path(
    post,
    path = "/v1/tokens",
    tag = "tokens",
    security(("token" = [])),
    request_body = NewTokenRequest,
    responses(
        (status = CREATED, body = CreatedToken),
        (status = BAD_REQUEST, description = "`malformed_request`, e.g. `webhook` missing or misplaced.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn create_token(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Json(request): Json<NewTokenRequest>,
) -> Result<(StatusCode, Json<CreatedToken>), ApiError> {
    let name = token_name(request.name)?;
    let scope = match (request.scope, request.webhook) {
        (ScopeName::Admin, None) => Scope::Admin,
        (ScopeName::Agent, None) => Scope::Agent,
        (ScopeName::Webhook, Some(binding)) => Scope::Webhook {
            workflow_id: binding.workflow_id,
            trigger_id: binding.trigger_id,
        },
        (ScopeName::Webhook, None) => {
            return Err(ApiError::malformed_request(
                "the `webhook` Scope needs `webhook`: the `workflow_id` and `trigger_id` it \
                 may start Runs from",
            ));
        }
        (ScopeName::Admin | ScopeName::Agent, Some(_)) => {
            return Err(ApiError::malformed_request(
                "`webhook` is only for the `webhook` Scope",
            ));
        }
    };
    let (value, hash) = TokenValue::generate().map_err(|e| ApiError::internal(&e))?;
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let token = PgTokenRepository
        .create(
            &mut conn,
            &NewToken {
                name,
                hash,
                scope,
                user_id: caller.user_id,
                created_at: state.clock.now(),
            },
        )
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tracing::info!(token_id = %token.id, scope = token.scope.as_str(), "token created");
    Ok(created(token, value))
}

/// Every Token, revoked ones included, without their values. Needs an `admin` Token.
#[utoipa::path(
    get,
    path = "/v1/tokens",
    tag = "tokens",
    security(("token" = [])),
    responses((status = OK, body = TokenList), AuthErrors)
)]
pub(crate) async fn list_tokens(
    _: Admin,
    State(state): State<ApiState>,
) -> Result<Json<TokenList>, ApiError> {
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let tokens = PgTokenRepository
        .list(&mut conn)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    Ok(Json(TokenList {
        tokens: tokens.into_iter().map(TokenInfo::from).collect(),
    }))
}

/// Revokes a Token at once; revoking it again changes nothing. Needs an `admin` Token, which
/// may revoke itself.
#[utoipa::path(
    post,
    path = "/v1/tokens/{token_id}/revoke",
    tag = "tokens",
    security(("token" = [])),
    params(("token_id" = Uuid, Path, description = "The Token's `id`.")),
    responses(
        (status = OK, body = TokenInfo),
        (status = NOT_FOUND, description = "`token_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn revoke_token(
    _: Admin,
    State(state): State<ApiState>,
    Path(token_id): Path<Uuid>,
) -> Result<Json<TokenInfo>, ApiError> {
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let token = PgTokenRepository
        .revoke(&mut conn, TokenId::from_uuid(token_id), state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                ErrorCode::TokenNotFound,
                format!("no Token has id {token_id}"),
            )
        })?;
    tracing::info!(token_id = %token.id, "token revoked");
    Ok(Json(token.into()))
}

/// The Token the request is made with, so a client can tell which controls its Scope allows.
/// Open to `admin` and `agent` Tokens, the two the UI signs in with.
#[utoipa::path(
    get,
    path = "/v1/tokens/me",
    tag = "tokens",
    security(("token" = [])),
    responses((status = OK, body = TokenInfo), AuthErrors)
)]
pub(crate) async fn current_token(Agent(token): Agent) -> Json<TokenInfo> {
    Json(token.into())
}

fn token_name(name: String) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Err(ApiError::malformed_request(format!(
            "`name` must be 1 to {MAX_NAME_LEN} characters"
        )));
    }
    Ok(name.to_owned())
}

pub(crate) fn created(token: Token, value: TokenValue) -> (StatusCode, Json<CreatedToken>) {
    (
        StatusCode::CREATED,
        Json(CreatedToken {
            token: token.into(),
            value: value.into_string(),
        }),
    )
}

impl From<Token> for TokenInfo {
    fn from(token: Token) -> Self {
        let (scope, webhook) = match token.scope {
            Scope::Admin => (ScopeName::Admin, None),
            Scope::Agent => (ScopeName::Agent, None),
            Scope::Webhook {
                workflow_id,
                trigger_id,
            } => (
                ScopeName::Webhook,
                Some(WebhookBinding {
                    workflow_id,
                    trigger_id,
                }),
            ),
        };
        Self {
            id: token.id.as_uuid(),
            name: token.name,
            scope,
            webhook,
            created_at: token.created_at,
            last_used_at: token.last_used_at,
            revoked_at: token.revoked_at,
        }
    }
}
