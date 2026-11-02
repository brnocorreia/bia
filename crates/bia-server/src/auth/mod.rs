//! Tokens and the Scopes routes require (ADR-0008, ADR-0043).
//!
//! A request authenticates with `Authorization: Bearer <Token>` or, for the browser, with the
//! Session cookie; the header wins when both are present. A request authenticated by the
//! cookie that uses an unsafe method must also carry an `Origin` the server knows, since a
//! browser sends the cookie whatever page made the request (ADR-0064).
//!
//! A handler declares what it requires by taking [`Admin`], [`Agent`] or [`Webhook`] as its
//! first argument, before anything that reads the request, so an unauthenticated caller gets
//! 401 rather than a validation error. A handler that takes none is public; a test checks that
//! only the probes and setup are. The webhook route takes `Option<Webhook>`, since a
//! Trigger with `hmac` is authenticated by its signature instead (ADR-0058).

use axum::extract::{FromRequestParts, OptionalFromRequestParts, RawPathParams};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use bia_store::token::{Origin, PgTokenRepository, Scope, Token, TokenRepository};
use utoipa::IntoResponses;

use crate::{ApiError, ApiState, ErrorBody, ErrorCode};

pub(crate) mod passkey;
pub(crate) mod password;
pub(crate) mod session;
mod setup;
mod throttle;
mod value;

pub(crate) use setup::CodeCheck;
pub use setup::SetupCode;
pub use throttle::Wait;
pub(crate) use throttle::{ClientAddress, SignInThrottle, TokioWait};
pub(crate) use value::{TokenValue, secret_hash};

/// Marks a request that reached the API through the MCP server, which is what lets a Token
/// issued for `/mcp` only be used. It is an extension on a request built in-process, so nothing
/// a client sends can carry it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ViaMcp;

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
        let token = authenticate(parts, state).await?;
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
        let token = authenticate(parts, state).await?;
        match token.scope {
            Scope::Admin | Scope::Agent => Ok(Self(token)),
            Scope::Webhook { .. } => Err(insufficient_scope(&token, "an `admin` or `agent` Token")),
        }
    }
}

impl FromRequestParts<ApiState> for Webhook {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &ApiState) -> Result<Self, ApiError> {
        let token = authenticate(parts, state).await?;
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

/// The live Token the request carries, in the header or the Session cookie.
async fn authenticate(parts: &Parts, state: &ApiState) -> Result<Token, ApiError> {
    let (presented, by_cookie) = if parts.headers.contains_key(header::AUTHORIZATION) {
        (bearer(&parts.headers)?, false)
    } else if let Some(value) = session::cookie_value(&parts.headers) {
        (value, true)
    } else {
        return Err(missing_token());
    };
    if !value::is_token_value(presented) {
        return Err(unknown_token());
    }
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let token = PgTokenRepository
        .authenticate(&mut conn, value::hash(presented), state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(unknown_token)?;
    if let Some(audience) = &token.audience
        && !state
            .config
            .audience_allows(audience, parts.extensions.get::<ViaMcp>().is_some())
    {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::WrongAudience,
            "this Token was issued for the MCP endpoint (`/mcp`) only",
        ));
    }
    if by_cookie {
        // A cookie is only ever a Session; any other Token in one is a mistake or an attack.
        if token.origin != Origin::Session {
            return Err(unknown_token());
        }
        check_origin(parts, state)?;
    }
    Ok(token)
}

/// Refuses an unsafe request whose `Origin` is not the server's own or the UI's.
fn check_origin(parts: &Parts, state: &ApiState) -> Result<(), ApiError> {
    if matches!(
        parts.method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    ) {
        return Ok(());
    }
    let allowed = parts
        .headers
        .get(header::ORIGIN)
        .and_then(|origin| origin.to_str().ok())
        .is_some_and(|origin| state.config.is_own_origin(origin));
    if allowed {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::OriginMismatch,
            "a request made with the Session cookie must carry an `Origin` header equal to the \
             origin of `public_url` or `ui_url`",
        ))
    }
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

/// The failures every route that needs a Token can answer with.
#[derive(IntoResponses)]
#[allow(dead_code)]
pub(crate) enum AuthErrors {
    /// No Token, or one that is unknown, revoked or expired; `code` is `missing_token` or
    /// `invalid_token`.
    #[response(status = UNAUTHORIZED)]
    Unauthenticated(ErrorBody),
    /// The Token's Scope does not allow this route (`insufficient_scope`), or a request made
    /// with the Session cookie carries no acceptable `Origin` (`origin_mismatch`).
    #[response(status = FORBIDDEN)]
    Forbidden(ErrorBody),
}
