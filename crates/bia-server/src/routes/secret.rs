//! Secrets of the internal Secret Provider: written and deleted by `admin`, listed by name by
//! `admin` and `agent`. No route returns a value (ADR-0017, ADR-0049).

use axum::extract::State;
use bia_store::secret::{
    MasterKey, PgSecretRepository, SecretInfo as StoredInfo, SecretName, SecretRepository,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{Admin, Agent, AuthErrors};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path};
use axum::http::StatusCode;

/// The value of a Secret, to store.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PutSecretRequest {
    /// Encrypted before it is stored and never returned by any route. At least one character.
    value: String,
}

// Not derived, so the value cannot reach a log through `Debug`.
impl std::fmt::Debug for PutSecretRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PutSecretRequest(..)")
    }
}

/// A Secret, without its value.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SecretInfo {
    /// Read as `secrets.NAME` in a Step's `params`.
    #[schema(example = "API_TOKEN")]
    name: String,
    created_at: DateTime<Utc>,
    /// When the value was last set.
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SecretList {
    /// Every Secret by name.
    secrets: Vec<SecretInfo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SecretDeleted {
    name: String,
}

/// Creates the Secret, or replaces its value. Needs an `admin` Token. The value is encrypted
/// under the server's `BIA_MASTER_KEY` and cannot be read back, only replaced.
#[utoipa::path(
    put,
    path = "/v1/secrets/{name}",
    tag = "secrets",
    security(("token" = [])),
    params(("name" = String, Path, description = "Letters, digits and `_`, not starting with a digit.")),
    request_body = PutSecretRequest,
    responses(
        (status = OK, body = SecretInfo),
        (status = BAD_REQUEST, description = "`malformed_request`: the name or the value is not valid.", body = ErrorBody),
        (status = SERVICE_UNAVAILABLE, description = "`master_key_missing`: the server has no `BIA_MASTER_KEY`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn put_secret(
    _: Admin,
    State(state): State<ApiState>,
    Path(name): Path<String>,
    Json(request): Json<PutSecretRequest>,
) -> Result<Json<SecretInfo>, ApiError> {
    let name = secret_name(name)?;
    if request.value.is_empty() {
        return Err(ApiError::malformed_request("`value` must not be empty"));
    }
    let sealed = master_key(&state)?
        .seal(&name, &request.value)
        .map_err(|e| ApiError::internal(&e))?;
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let info = PgSecretRepository
        .put(&mut conn, &name, &sealed, state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tracing::info!(secret = %name, "secret set");
    Ok(Json(info.into()))
}

/// Every Secret by name, without values. Needs an `admin` or `agent` Token: an agent writing a
/// Definition needs the names to reference.
#[utoipa::path(
    get,
    path = "/v1/secrets",
    tag = "secrets",
    security(("token" = [])),
    responses((status = OK, body = SecretList), AuthErrors)
)]
pub(crate) async fn list_secrets(
    _: Agent,
    State(state): State<ApiState>,
) -> Result<Json<SecretList>, ApiError> {
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let secrets = PgSecretRepository
        .list(&mut conn)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    Ok(Json(SecretList {
        secrets: secrets.into_iter().map(SecretInfo::from).collect(),
    }))
}

/// Deletes a Secret. Needs an `admin` Token. Steps that read it then fail with
/// `unknown_secret`.
#[utoipa::path(
    delete,
    path = "/v1/secrets/{name}",
    tag = "secrets",
    security(("token" = [])),
    params(("name" = String, Path, description = "The Secret's name.")),
    responses(
        (status = OK, body = SecretDeleted),
        (status = NOT_FOUND, description = "`secret_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn delete_secret(
    _: Admin,
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<SecretDeleted>, ApiError> {
    let name = secret_name(name)?;
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    if !PgSecretRepository
        .delete(&mut conn, &name)
        .await
        .map_err(|e| ApiError::internal(&e))?
    {
        return Err(not_found(&name));
    }
    tracing::info!(secret = %name, "secret deleted");
    Ok(Json(SecretDeleted {
        name: name.to_string(),
    }))
}

fn secret_name(name: String) -> Result<SecretName, ApiError> {
    SecretName::new(name).map_err(|e| ApiError::malformed_request(e.to_string()))
}

fn master_key(state: &ApiState) -> Result<&MasterKey, ApiError> {
    state.config.master_key.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::MasterKeyMissing,
            "the server cannot store Secrets: start it with BIA_MASTER_KEY set",
        )
    })
}

fn not_found(name: &SecretName) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        ErrorCode::SecretNotFound,
        format!("no Secret is named '{name}'"),
    )
}

impl From<StoredInfo> for SecretInfo {
    fn from(info: StoredInfo) -> Self {
        Self {
            name: info.name.to_string(),
            created_at: info.created_at,
            updated_at: info.updated_at,
        }
    }
}
