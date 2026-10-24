//! Liveness and readiness probes, for orchestrators and load balancers.

use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;
use utoipa::ToSchema;

use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json};

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Status {
    #[schema(example = "ok")]
    status: &'static str,
}

/// Answers while the process serves HTTP, whatever the state of the database.
#[utoipa::path(get, path = "/health", tag = "probes", responses((status = OK, body = Status)))]
pub(crate) async fn health() -> Json<Status> {
    Json(Status { status: "ok" })
}

/// Answers once the database is reachable and every migration this build needs is applied.
#[utoipa::path(
    get,
    path = "/ready",
    tag = "probes",
    responses(
        (status = OK, body = Status),
        (status = SERVICE_UNAVAILABLE, description = "Not ready; `code` is `not_ready`.", body = ErrorBody),
    )
)]
pub(crate) async fn ready(State(state): State<ApiState>) -> Result<Json<Status>, ApiError> {
    let not_ready = |message: String| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::NotReady,
            message,
        )
    };
    match bia_store::pending_migrations(&state.pool).await {
        Ok(pending) if pending.is_empty() => Ok(Json(Status { status: "ready" })),
        Ok(pending) => Err(not_ready(format!(
            "{} migration(s) are pending; run `bia migrate`",
            pending.len()
        ))),
        Err(error) => {
            tracing::warn!(%error, "readiness check failed");
            Err(not_ready("the database is unreachable".to_owned()))
        }
    }
}
