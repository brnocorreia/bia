//! First-run setup: the Setup Link's code and a password create the install's User (ADR-0062).
//! Until sign-in exists, setup also returns an admin Token, so the API stays usable.

use argon2::{Argon2, PasswordHasher};
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use bia_store::token::{NewToken, PgTokenRepository, Scope, TokenRepository};
use bia_store::user::{PasswordHash, PgUserRepository, UserRepository};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Acquire;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{CodeCheck, TokenValue};
use crate::routes::token::{CreatedToken, created};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json};

/// Counted in characters. No other rule: length is what makes a password hard to guess.
const MIN_PASSWORD_LEN: usize = 12;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetupRequest {
    /// The `code` in the Setup Link that `bia serve` logs.
    #[schema(example = "1f2e3d4c-00112233445566778899aabbccddeeff")]
    code: String,
    /// At least 12 characters.
    password: String,
}

/// The User setup created, and an admin Token for it.
// No `Debug`, so the Token's value cannot reach a log through one.
#[derive(Serialize, ToSchema)]
pub(crate) struct SetupDone {
    user: UserInfo,
    /// An admin Token to call the API with until sign-in exists. Shown only in this response.
    token: CreatedToken,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UserInfo {
    id: Uuid,
    created_at: DateTime<Utc>,
}

/// Sets the password of the install's User, creating it on a fresh install, with the code
/// from the Setup Link that `bia serve` logs. Public, since no one can sign in yet. Refuses
/// once a User has a password; `bia serve --reset-admin` clears it and logs a new link.
#[utoipa::path(
    post,
    path = "/v1/setup",
    tag = "setup",
    request_body = SetupRequest,
    responses(
        (status = CREATED, body = SetupDone),
        (status = BAD_REQUEST, description = "`malformed_request`: the body is not `{code, password}`.", body = ErrorBody),
        (status = FORBIDDEN, description = "`invalid_setup_code`: the code is wrong; `setup_code_expired`: it is from before the server last started, so use the link it logged since.", body = ErrorBody),
        (status = CONFLICT, description = "`already_set_up`: a User has a password.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`password_too_short`: the password has fewer than 12 characters.", body = ErrorBody),
    )
)]
pub(crate) async fn set_up(
    State(state): State<ApiState>,
    request: Result<axum::Json<SetupRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SetupDone>), ApiError> {
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
    if request.password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::PasswordTooShort,
            format!("the password must be at least {MIN_PASSWORD_LEN} characters"),
        ));
    }
    let password = hash_password(request.password).await?;
    let (value, hash) = TokenValue::generate().map_err(|e| ApiError::internal(&e))?;
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
    let token = PgTokenRepository
        .create(
            &mut tx,
            &NewToken {
                name: "setup".to_owned(),
                hash,
                scope: Scope::Admin,
                user_id: user.id,
                created_at: now,
            },
        )
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    tracing::warn!(user_id = %user.id, token_id = %token.id, "setup done");
    let (status, Json(token)) = created(token, value);
    Ok((
        status,
        Json(SetupDone {
            user: UserInfo {
                id: user.id.as_uuid(),
                created_at: user.created_at,
            },
            token,
        }),
    ))
}

/// argon2id with the crate's defaults, which follow OWASP's recommendation, off the async
/// runtime since it is slow on purpose.
async fn hash_password(password: String) -> Result<PasswordHash, ApiError> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| PasswordHash(hash.to_string()))
    })
    .await
    .map_err(|e| ApiError::internal(&e))?
    .map_err(|e| ApiError::internal(&e))
}
