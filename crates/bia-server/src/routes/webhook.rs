//! Webhook Triggers: a request to `/hooks/{workflow_id}/{trigger_id}` starts a Run of the
//! Active Version with the body as `input`. Callers authenticate with the Trigger's bound
//! Token, or with an HMAC-SHA256 signature when the Trigger sets `hmac` (ADR-0058).

use axum::body::Bytes;
use axum::extract::State;
use axum::extract::rejection::BytesRejection;
use axum::http::{HeaderMap, StatusCode};
use bia_core::dsl::{Hmac, TriggerKind, WorkflowId};
use bia_engine::{InternalSecrets, SecretResolver};
use bia_store::version::{PgVersionRepository, VersionNumber, VersionRepository};
use bia_store::workflow::{PgWorkflowRepository, WorkflowRepository};
use hmac::{KeyInit, Mac};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::Sha256;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use super::run::{check_input, engine_error};
use super::workflow::{acquire, store_error};
use crate::auth::{self, AuthErrors, Webhook};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path};

/// Headers whose name contains one of these may carry a credential, so they stay out of the
/// Run's `trigger`.
const CREDENTIAL_WORDS: [&str; 9] = [
    "auth",
    "token",
    "secret",
    "signature",
    "password",
    "session",
    "credential",
    "cookie",
    "key",
];

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WebhookAccepted {
    /// The Run the request started. It is `pending` until a Worker picks it up.
    run_id: Uuid,
}

/// Starts a Run of the Workflow's Active Version from one of its webhook Triggers, with the
/// JSON body (`{}` when empty) as `input`, checked against the `input_schema`. A Trigger without
/// `hmac` needs its bound `webhook` Token or an `admin` Token as `Authorization: Bearer`; one
/// with `hmac` needs the HMAC-SHA256 of the raw body, as hex optionally prefixed `sha256=`, in
/// its `hmac.header`. Without a Token, a path that names no webhook Trigger of an Active Version
/// answers `missing_token`, like a Trigger that needs one.
#[utoipa::path(
    post,
    path = "/hooks/{workflow_id}/{trigger_id}",
    tag = "webhooks",
    security(("token" = []), ()),
    params(
        ("workflow_id" = String, Path, description = "The Workflow's id."),
        ("trigger_id" = String, Path, description = "The webhook Trigger's id."),
    ),
    request_body(content = Object, description = "The Run input, as JSON.", content_type = "application/json"),
    responses(
        (status = ACCEPTED, body = WebhookAccepted),
        (status = BAD_REQUEST, description = "`malformed_request`: the body is not JSON.", body = ErrorBody),
        (status = NOT_FOUND, description = "`webhook_not_found`, to a caller with a Token.", body = ErrorBody),
        (status = PAYLOAD_TOO_LARGE, description = "`request_too_large`.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_run_input`.", body = ErrorBody),
        (status = SERVICE_UNAVAILABLE, description = "`hmac_secret_unavailable`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn trigger_webhook(
    token: Option<Webhook>,
    State(state): State<ApiState>,
    Path((workflow_id, trigger_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<WebhookAccepted>), ApiError> {
    let mut conn = acquire(&state).await?;
    let Some(found) = active_webhook(&mut conn, &workflow_id, &trigger_id).await? else {
        return Err(match token {
            Some(_) => ApiError::new(
                StatusCode::NOT_FOUND,
                ErrorCode::WebhookNotFound,
                format!(
                    "the Active Version of workflow `{workflow_id}` has no webhook Trigger \
                     `{trigger_id}`"
                ),
            ),
            None => auth::missing_token(),
        });
    };
    let body = body.map_err(body_error)?;
    match &found.hmac {
        Some(hmac) => verify_signature(&state, hmac, &headers, &body).await?,
        None if token.is_none() => return Err(auth::missing_token()),
        None => {}
    }

    let input = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        // serde's message can quote the body, so it is not passed on.
        serde_json::from_slice(&body)
            .map_err(|_| ApiError::malformed_request("the body is not JSON"))?
    };
    check_input(&found.definition, &input)?;

    let trigger = json!({
        "type": "webhook",
        "id": trigger_id,
        "received_at": state.clock.now(),
        "headers": trigger_headers(&headers, found.hmac.as_ref()),
    });
    let run = bia_engine::start_run(
        &mut conn,
        &found.workflow_id,
        found.version,
        input,
        trigger,
        state.clock.now(),
    )
    .await
    .map_err(engine_error)?;
    tracing::info!(
        run_id = %run.id,
        workflow = %found.workflow_id,
        trigger = %trigger_id,
        version = found.version.get(),
        "run started by webhook"
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(WebhookAccepted {
            run_id: run.id.as_uuid(),
        }),
    ))
}

struct ActiveWebhook {
    workflow_id: WorkflowId,
    version: VersionNumber,
    definition: bia_core::dsl::Definition,
    hmac: Option<Hmac>,
}

/// The webhook Trigger `trigger_id` of the Workflow's Active Version, if both exist.
async fn active_webhook(
    conn: &mut PgConnection,
    workflow_id: &str,
    trigger_id: &str,
) -> Result<Option<ActiveWebhook>, ApiError> {
    let Ok(workflow_id) = WorkflowId::new(workflow_id) else {
        return Ok(None);
    };
    let Some(workflow) = PgWorkflowRepository
        .get(conn, &workflow_id)
        .await
        .map_err(store_error)?
    else {
        return Ok(None);
    };
    let Some(version) = workflow.active_version else {
        return Ok(None);
    };
    let Some(stored) = PgVersionRepository
        .get(conn, &workflow_id, version)
        .await
        .map_err(store_error)?
    else {
        return Ok(None);
    };
    let hmac = stored
        .definition
        .triggers
        .iter()
        .find_map(|trigger| match &trigger.kind {
            TriggerKind::Webhook(webhook) if trigger.id.as_str() == trigger_id => {
                Some(webhook.hmac.clone())
            }
            _ => None,
        });
    Ok(hmac.map(|hmac| ActiveWebhook {
        workflow_id,
        version,
        definition: stored.definition,
        hmac,
    }))
}

async fn verify_signature(
    state: &ApiState,
    hmac: &Hmac,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), ApiError> {
    let Some(presented) = headers.get(hmac.header.as_str()) else {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::MissingSignature,
            format!(
                "this webhook needs an HMAC-SHA256 signature in `{}`",
                hmac.header
            ),
        ));
    };
    let key = hmac_key(state, &hmac.secret).await?;
    let signature = presented
        .to_str()
        .ok()
        .map(|value| {
            let value = value.trim();
            value.strip_prefix("sha256=").unwrap_or(value)
        })
        .and_then(|hex| hex::decode(hex).ok());
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key.as_bytes())
        .unwrap_or_else(|_| unreachable!("HMAC takes a key of any length"));
    mac.update(body);
    // `verify_slice` compares in constant time.
    match signature {
        Some(signature) if mac.verify_slice(&signature).is_ok() => Ok(()),
        _ => Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::InvalidSignature,
            format!("the signature in `{}` does not match the body", hmac.header),
        )),
    }
}

/// The value of the Secret `secrets.NAME` names. Failures are logged by name only and answered
/// with a fixed message, so neither the value nor provider errors reach the caller.
async fn hmac_key(state: &ApiState, reference: &str) -> Result<String, ApiError> {
    let unavailable = || {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::HmacSecretUnavailable,
            "the webhook's HMAC Secret is unavailable; see the server's logs",
        )
    };
    let Some(name) = reference.strip_prefix("secrets.") else {
        tracing::warn!(
            secret = reference,
            "webhook hmac.secret is not `secrets.NAME`"
        );
        return Err(unavailable());
    };
    let secrets = InternalSecrets::new(state.pool.clone(), state.config.master_key.clone());
    match secrets.resolve(name).await {
        Ok(Some(key)) => Ok(key),
        Ok(None) => {
            tracing::warn!(secret = name, "webhook HMAC Secret does not exist");
            Err(unavailable())
        }
        Err(error) => {
            tracing::warn!(
                secret = name,
                error = error.message(),
                "webhook HMAC Secret unreadable"
            );
            Err(unavailable())
        }
    }
}

/// The request's headers for the Run's `trigger`, without any that may carry a credential.
fn trigger_headers(headers: &HeaderMap, hmac: Option<&Hmac>) -> Map<String, Value> {
    let signature = hmac.map(|hmac| hmac.header.to_ascii_lowercase());
    let mut kept = Map::new();
    for name in headers.keys() {
        let name = name.as_str();
        if signature.as_deref() == Some(name)
            || CREDENTIAL_WORDS.iter().any(|word| name.contains(word))
        {
            continue;
        }
        let values: Vec<&str> = headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect();
        if !values.is_empty() {
            kept.insert(name.to_owned(), Value::String(values.join(", ")));
        }
    }
    kept
}

fn body_error(rejection: BytesRejection) -> ApiError {
    let status = rejection.status();
    let code = if status == StatusCode::PAYLOAD_TOO_LARGE {
        ErrorCode::RequestTooLarge
    } else {
        ErrorCode::MalformedRequest
    };
    ApiError::new(status, code, rejection.body_text())
}
