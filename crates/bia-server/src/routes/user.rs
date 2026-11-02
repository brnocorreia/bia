//! The User's own settings: their password (ADR-0064).

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use bia_store::token::{PgTokenRepository, TokenRepository};
use bia_store::user::{PgUserRepository, User, UserRepository};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Acquire;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::{Admin, AuthErrors, ClientAddress, password};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json};

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UserInfo {
    id: Uuid,
    created_at: DateTime<Utc>,
}

impl From<User> for UserInfo {
    fn from(user: User) -> Self {
        Self {
            id: user.id.as_uuid(),
            created_at: user.created_at,
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangePasswordRequest {
    /// The password being replaced.
    current: String,
    /// The new password: at least 12 characters.
    new: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PasswordChanged {
    /// How many other Sessions it signed out. The caller's own Session stays. OAuth Grants are
    /// not affected.
    revoked_sessions: u64,
}

/// Replaces the password and revokes every other Session, so a stolen one stops working. Needs
/// an `admin` Token and the current password, and counts wrong ones towards sign-in
/// throttling.
#[utoipa::path(
    post,
    path = "/v1/user/password",
    tag = "user",
    security(("token" = []), ("session" = [])),
    request_body = ChangePasswordRequest,
    responses(
        (status = OK, body = PasswordChanged),
        (status = BAD_REQUEST, description = "`malformed_request`: the body is not `{current, new}`.", body = ErrorBody),
        (status = UNAUTHORIZED, description = "`invalid_credentials`: `current` is wrong; `missing_token` or `invalid_token`.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`password_too_short`: `new` has fewer than 12 characters.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn change_password(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    ClientAddress(address): ClientAddress,
    request: Result<axum::Json<ChangePasswordRequest>, JsonRejection>,
) -> Result<Json<PasswordChanged>, ApiError> {
    // axum's rejection text can quote the body, which holds passwords.
    let axum::Json(request) = request.map_err(|rejection| {
        ApiError::new(
            rejection.status(),
            ErrorCode::MalformedRequest,
            "the body must be a JSON object with the strings `current` and `new`",
        )
    })?;
    if request.new.chars().count() < password::MIN_LEN {
        return Err(password::too_short());
    }
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let credentials = PgUserRepository
        .credentials(&mut conn)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .filter(|credentials| credentials.user.id == caller.user_id)
        .ok_or_else(password::invalid_credentials)?;
    let subject = state.throttle(caller.user_id, address).await;
    if !password::verify(request.current, credentials.password).await? {
        return Err(password::invalid_credentials());
    }
    state.sign_ins.succeeded(subject);
    let new = password::hash(request.new).await?;
    let now = state.clock.now();
    let mut tx = conn.begin().await.map_err(|e| ApiError::internal(&e))?;
    PgUserRepository
        .set_password(&mut tx, caller.user_id, &new)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let revoked_sessions = PgTokenRepository
        .revoke_sessions_except(&mut tx, caller.user_id, Some(caller.id), now)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    tracing::warn!(user_id = %caller.user_id, revoked_sessions, "password changed");
    Ok(Json(PasswordChanged { revoked_sessions }))
}
