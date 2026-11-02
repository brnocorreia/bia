//! Sign-in and sign-out: a Session is an `admin` Token in an `HttpOnly` cookie (ADR-0064).

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderValue, StatusCode, header};
use bia_store::token::{Origin, PgTokenRepository, TokenRepository};
use bia_store::user::{PgUserRepository, UserRepository};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{Agent, AuthErrors, ClientAddress, password, session};
use crate::routes::token::TokenInfo;
use crate::routes::user::UserInfo;
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json};

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignInRequest {
    password: String,
}

/// A Session just started. Its value is only in the `Set-Cookie` header.
// No `Debug`, so nothing here can reach a log through one.
#[derive(Serialize, ToSchema)]
pub(crate) struct SignedIn {
    user: UserInfo,
    /// The Session, which is a Token with origin `session`.
    session: TokenInfo,
}

/// Signs in with the password and sets the `bia_session` cookie: `HttpOnly`, `SameSite=Lax`,
/// `Secure` when `public_url` is HTTPS, and no `Domain`. The Session lasts 14 days without use
/// and 90 days at most. After five failed attempts each further one waits longer, up to a
/// minute, per User and client address. Public, since it is how a browser gets a Token.
#[utoipa::path(
    post,
    path = "/v1/session",
    tag = "session",
    request_body = SignInRequest,
    responses(
        (status = CREATED, body = SignedIn, headers(("Set-Cookie" = String, description = "`bia_session=<value>`"))),
        (status = BAD_REQUEST, description = "`malformed_request`: the body is not `{password}`.", body = ErrorBody),
        (status = UNAUTHORIZED, description = "`invalid_credentials`: the password is wrong, or no User has one yet.", body = ErrorBody),
    )
)]
pub(crate) async fn sign_in(
    State(state): State<ApiState>,
    ClientAddress(address): ClientAddress,
    request: Result<axum::Json<SignInRequest>, JsonRejection>,
) -> Result<
    (
        StatusCode,
        [(header::HeaderName, HeaderValue); 1],
        Json<SignedIn>,
    ),
    ApiError,
> {
    // axum's rejection text can quote the body, which holds the password.
    let axum::Json(request) = request.map_err(|rejection| {
        ApiError::new(
            rejection.status(),
            ErrorCode::MalformedRequest,
            "the body must be a JSON object with the string `password`",
        )
    })?;
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let credentials = PgUserRepository
        .credentials(&mut conn)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(password::invalid_credentials)?;
    let subject = state.throttle(credentials.user.id, address).await;
    if !password::verify(request.password, credentials.password).await? {
        return Err(password::invalid_credentials());
    }
    state.sign_ins.succeeded(subject);
    let (token, value) = session::start(&state, &mut conn, credentials.user.id).await?;
    tracing::info!(user_id = %token.user_id, session_id = %token.id, "signed in");
    Ok((
        StatusCode::CREATED,
        [(
            header::SET_COOKIE,
            session::set_cookie(&value, state.config.secure_cookies()),
        )],
        Json(SignedIn {
            user: credentials.user.into(),
            session: token.into(),
        }),
    ))
}

/// Revokes the Session the request is made with and clears its cookie. Needs a Session.
#[utoipa::path(
    delete,
    path = "/v1/session",
    tag = "session",
    security(("session" = []), ("token" = [])),
    responses(
        (status = OK, body = TokenInfo, headers(("Set-Cookie" = String, description = "Clears `bia_session`"))),
        (status = CONFLICT, description = "`not_a_session`: the request is made with a Token that is not a Session.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn sign_out(
    Agent(caller): Agent,
    State(state): State<ApiState>,
) -> Result<([(header::HeaderName, HeaderValue); 1], Json<TokenInfo>), ApiError> {
    if caller.origin != Origin::Session {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::NotASession,
            "this request is not made with a Session; revoke a Token with \
             `POST /v1/tokens/{id}/revoke`",
        ));
    }
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let revoked = PgTokenRepository
        .revoke(&mut conn, caller.id, state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?
        .unwrap_or(caller);
    tracing::info!(session_id = %revoked.id, "signed out");
    Ok((
        [(
            header::SET_COOKIE,
            session::clear_cookie(state.config.secure_cookies()),
        )],
        Json(revoked.into()),
    ))
}
