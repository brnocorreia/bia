//! First-run setup: the Setup Link's code and a password create the install's User and start
//! its first Session (ADR-0062, ADR-0064).

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderValue, StatusCode, header};
use bia_store::user::{PgUserRepository, UserRepository};
use serde::{Deserialize, Serialize};
use sqlx::Acquire;
use utoipa::ToSchema;

use crate::auth::{CodeCheck, password, session};
use crate::routes::user::UserInfo;
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json};

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetupRequest {
    /// The `code` in the Setup Link that `bia serve` logs.
    #[schema(example = "1f2e3d4c-00112233445566778899aabbccddeeff")]
    code: String,
    /// At least 12 characters.
    password: String,
}

/// The User setup created. The Session it started is in the `Set-Cookie` header.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SetupDone {
    user: UserInfo,
}

/// Sets the password of the install's User, creating it on a fresh install, with the code
/// from the Setup Link that `bia serve` logs, and signs that User in: the response sets the
/// `bia_session` cookie, as `POST /v1/session` does. Public, since no one can sign in yet.
/// Refuses once a User has a password; `bia serve --reset-admin` clears it and logs a new link.
#[utoipa::path(
    post,
    path = "/v1/setup",
    tag = "setup",
    request_body = SetupRequest,
    responses(
        (status = CREATED, body = SetupDone, headers(("Set-Cookie" = String, description = "`bia_session=<value>`"))),
        (status = BAD_REQUEST, description = "`malformed_request`: the body is not `{code, password}`.", body = ErrorBody),
        (status = FORBIDDEN, description = "`invalid_setup_code`: the code is wrong; `setup_code_expired`: it is from before the server last started, so use the link it logged since.", body = ErrorBody),
        (status = CONFLICT, description = "`already_set_up`: a User has a password.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`password_too_short`: the password has fewer than 12 characters.", body = ErrorBody),
    )
)]
pub(crate) async fn set_up(
    State(state): State<ApiState>,
    request: Result<axum::Json<SetupRequest>, JsonRejection>,
) -> Result<
    (
        StatusCode,
        [(header::HeaderName, HeaderValue); 1],
        Json<SetupDone>,
    ),
    ApiError,
> {
    // axum's rejection text can quote the body, which holds the password.
    let axum::Json(request) = request.map_err(|rejection| {
        ApiError::new(
            rejection.status(),
            ErrorCode::MalformedRequest,
            "the body must be a JSON object with the strings `code` and `password`",
        )
    })?;
    match state
        .config
        .setup_code
        .as_ref()
        .map(|code| code.check(&request.code))
    {
        Some(CodeCheck::Valid) => {}
        Some(CodeCheck::Expired) => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                ErrorCode::SetupCodeExpired,
                "this Setup Link is from before the server last started; use the one it logged \
                 since",
            ));
        }
        Some(CodeCheck::Wrong) | None => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                ErrorCode::InvalidSetupCode,
                "the setup code is wrong; copy it from the Setup Link `bia serve` logged",
            ));
        }
    }
    if request.password.chars().count() < password::MIN_LEN {
        return Err(password::too_short());
    }
    let password = password::hash(request.password).await?;
    let now = state.clock.now();
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let mut tx = conn.begin().await.map_err(|e| ApiError::internal(&e))?;
    let user = PgUserRepository
        .set_up(&mut tx, &password, now)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::AlreadySetUp,
                "setup is done; `bia serve --reset-admin` starts it again",
            )
        })?;
    let (token, value) = session::start(&state, &mut tx, user.id).await?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    tracing::warn!(user_id = %user.id, session_id = %token.id, "setup done");
    Ok((
        StatusCode::CREATED,
        [(
            header::SET_COOKIE,
            session::set_cookie(&value, state.config.secure_cookies()),
        )],
        Json(SetupDone { user: user.into() }),
    ))
}
