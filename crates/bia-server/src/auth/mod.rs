//! Bearer Tokens and the Scopes routes require (ADR-0008, ADR-0043).
//!
//! A handler declares what it requires by taking [`Admin`], [`Agent`] or [`Webhook`] as its
//! first argument, before anything that reads the request, so an unauthenticated caller gets
//! 401 rather than a validation error. A handler that takes none is public; a test checks that
//! only the probes and bootstrapping are. The webhook route takes `Option<Webhook>`, since a
//! Trigger with `hmac` is authenticated by its signature instead (ADR-0058).

use std::fmt;

use axum::extract::{FromRequestParts, OptionalFromRequestParts, RawPathParams};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use bia_store::token::{PgTokenRepository, Scope, Token, TokenHash, TokenRepository};
use utoipa::IntoResponses;

use crate::{ApiError, ApiState, ErrorBody, ErrorCode};

mod value;

pub(crate) use value::TokenValue;

/// A request made with an `admin` Token.
#[derive(Debug)]
pub struct Admin(pub Token);

/// A request made with an `admin` or `agent` Token: what an agent may do, which is everything
/// but publishing, activating and administration.
#[derive(Debug)]
pub struct Agent(pub Token);

/// A request to `/hooks/{workflow_id}/{trigger_id}` made with the `webhook` Token bound to that
/// Trigger, or with an `admin` Token. The route must name both path parameters.
#[derive(Debug)]
pub struct Webhook(pub Token);

impl FromRequestParts<ApiState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &ApiState) -> Result<Self, ApiError> {
        let token = authenticate(&parts.headers, state).await?;
        match token.scope {
            Scope::Admin => Ok(Self(token)),
            Scope::Agent | Scope::Webhook { .. } => {
                Err(insufficient_scope(&token, "an `admin` Token"))
            }
        }
    }
}

impl FromRequestParts<ApiState> for Agent {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &ApiState) -> Result<Self, ApiError> {
        let token = authenticate(&parts.headers, state).await?;
        match token.scope {
            Scope::Admin | Scope::Agent => Ok(Self(token)),
            Scope::Webhook { .. } => Err(insufficient_scope(&token, "an `admin` or `agent` Token")),
        }
    }
}

impl FromRequestParts<ApiState> for Webhook {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &ApiState) -> Result<Self, ApiError> {
        let token = authenticate(&parts.headers, state).await?;
        let Scope::Webhook {
            workflow_id,
            trigger_id,
        } = &token.scope
        else {
            return match token.scope {
                Scope::Admin => Ok(Self(token)),
                _ => Err(insufficient_scope(&token, "this Trigger's `webhook` Token")),
            };
        };
        let params = RawPathParams::from_request_parts(parts, state)
            .await
            .map_err(|_| insufficient_scope(&token, "this Trigger's `webhook` Token"))?;
        let param = |name: &str| {
            params
                .iter()
                .find_map(|(key, value)| (key == name).then_some(value))
        };
        if param("workflow_id") == Some(workflow_id.as_str())
            && param("trigger_id") == Some(trigger_id.as_str())
        {
            Ok(Self(token))
        } else {
            Err(insufficient_scope(&token, "this Trigger's `webhook` Token"))
        }
    }
}

/// `None` when the request has no `Authorization` header; a Token it does carry must be valid
/// for this Trigger.
impl OptionalFromRequestParts<ApiState> for Webhook {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApiState,
    ) -> Result<Option<Self>, ApiError> {
        if !parts.headers.contains_key(header::AUTHORIZATION) {
            return Ok(None);
        }
        <Self as FromRequestParts<ApiState>>::from_request_parts(parts, state)
            .await
            .map(Some)
    }
}

/// The unrevoked Token the request carries.
async fn authenticate(headers: &HeaderMap, state: &ApiState) -> Result<Token, ApiError> {
    let presented = bearer(headers)?;
    if !value::is_token_value(presented) {
        return Err(unknown_token());
    }
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    PgTokenRepository
        .authenticate(&mut conn, value::hash(presented), state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(unknown_token)
}

/// The credential in `Authorization: Bearer <credential>`. Errors never quote it.
pub(crate) fn bearer(headers: &HeaderMap) -> Result<&str, ApiError> {
    let Some(authorization) = headers.get(header::AUTHORIZATION) else {
        return Err(missing_token());
    };
    authorization
        .to_str()
        .ok()
        .and_then(|value| {
            let (scheme, credential) = value.split_once(' ')?;
            scheme
                .eq_ignore_ascii_case("bearer")
                .then(|| credential.trim())
        })
        .filter(|credential| !credential.is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                ErrorCode::InvalidToken,
                "the `Authorization` header is not `Bearer <token>`",
            )
        })
}

pub(crate) fn missing_token() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::MissingToken,
        "this route needs a Token; send `Authorization: Bearer <token>`",
    )
}

fn unknown_token() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::InvalidToken,
        "the Token is unknown or revoked",
    )
}

fn insufficient_scope(token: &Token, needed: &str) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        ErrorCode::InsufficientScope,
        format!(
            "this route needs {needed}; this Token's Scope is `{}`",
            token.scope.as_str()
        ),
    )
}

/// The key that lets `POST /v1/tokens/bootstrap` mint the first admin Token. Only its hash is
/// kept in memory.
#[derive(Clone)]
pub struct BootstrapKey(TokenHash);

/// Shorter keys are refused, so the key cannot be guessed.
pub const MIN_BOOTSTRAP_KEY_LEN: usize = 32;

#[derive(Debug, thiserror::Error)]
#[error("the bootstrap key must be at least {MIN_BOOTSTRAP_KEY_LEN} characters")]
pub struct BootstrapKeyTooShort;

impl BootstrapKey {
    pub fn new(key: &str) -> Result<Self, BootstrapKeyTooShort> {
        if key.chars().count() < MIN_BOOTSTRAP_KEY_LEN {
            return Err(BootstrapKeyTooShort);
        }
        Ok(Self(value::hash(key)))
    }

    /// Comparing hashes keeps the comparison's timing independent of how much of the key a
    /// caller guessed right.
    pub(crate) fn matches(&self, presented: &str) -> bool {
        value::hash(presented) == self.0
    }
}

impl fmt::Debug for BootstrapKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BootstrapKey(..)")
    }
}

/// The failures every route that needs a Token can answer with.
#[derive(IntoResponses)]
#[allow(dead_code)]
pub(crate) enum AuthErrors {
    /// No Token, or one that is unknown or revoked; `code` is `missing_token` or
    /// `invalid_token`.
    #[response(status = UNAUTHORIZED)]
    Unauthenticated(ErrorBody),
    /// The Token's Scope does not allow this route; `code` is `insufficient_scope`.
    #[response(status = FORBIDDEN)]
    Forbidden(ErrorBody),
}
