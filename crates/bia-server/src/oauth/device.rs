//! The device authorization grant (RFC 8628) for `bia login --device`, on a machine without a
//! browser (ADR-0068).
//!
//! The CLI posts to [`device_authorization`] and gets a device code, which only it holds, and a
//! short user code to show. The person opens `{ui_url}/device` on another device, enters the
//! user code ([`lookup`]) and sees the same consent screen as any authorization ([`get_device_consent`],
//! [`approve_device`], [`deny_device`], which need a Session). Meanwhile the CLI polls the
//! token endpoint, which [`super::token`] answers from the store's [`poll_device`].
//!
//! [`poll_device`]: bia_store::oauth::OAuthRepository::poll_device

use axum::Form;
use axum::extract::State;
use axum::extract::rejection::FormRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bia_store::oauth::{
    DeviceAuthorization, NewDeviceAuthorization, OAuthRepository, PgOAuthRepository, Provenance,
    SecretHash,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::authorize;
use super::client::{self, ResolvedClient};
use super::consent::{Approval, ConsentClient, GrantScopeName, ProvenanceName, require_session};
use super::error::no_store;
use super::{DEVICE_CODE_PREFIX, DEVICE_LIFETIME, OAuthError, OAuthErrorBody};
use crate::auth::{Admin, AuthErrors, ClientAddress, TokenValue, secret_hash};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path};

/// The `grant_type` of a device polling the token endpoint.
pub(crate) const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// No vowels, so a code cannot spell a word, and no `Y`; 20 letters make 8 of them about 34
/// bits, which the Session requirement, the lookup throttle and the 10-minute life make
/// unguessable.
const USER_CODE_ALPHABET: &[u8; 20] = b"BCDFGHJKLMNPQRSTVWXZ";
const USER_CODE_LEN: usize = 8;

/// A new user code, written `XXXX-XXXX`.
fn generate_user_code() -> Result<String, getrandom::Error> {
    // 240 is the largest multiple of 20 below 256: a byte at or above it is discarded, so
    // every letter is equally likely.
    let mut letters = String::with_capacity(USER_CODE_LEN);
    while letters.len() < USER_CODE_LEN {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes)?;
        for byte in bytes.into_iter().filter(|byte| *byte < 240) {
            if letters.len() < USER_CODE_LEN {
                letters.push(char::from(
                    USER_CODE_ALPHABET[usize::from(byte) % USER_CODE_ALPHABET.len()],
                ));
            }
        }
    }
    let (head, tail) = letters.split_at(USER_CODE_LEN / 2);
    Ok(format!("{head}-{tail}"))
}

/// The hash the store keeps of a user code a person typed, ignoring case, hyphens and spaces;
/// `None` when it cannot be a user code.
fn user_code_hash(typed: &str) -> Option<SecretHash> {
    let letters: String = typed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let valid = letters.len() == USER_CODE_LEN
        && letters
            .bytes()
            .all(|letter| USER_CODE_ALPHABET.contains(&letter));
    valid.then(|| secret_hash(&letters))
}

/// What a device posts.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct DeviceAuthorizationRequest {
    /// Only `bia-cli`, the built-in client.
    client_id: Option<String>,
    /// `agent` (default) or `admin`; the person may approve less than is asked.
    scope: Option<String>,
    /// The audience (RFC 8707): the server's `public_url`, or `{public_url}/mcp`.
    resource: Option<String>,
}

/// What the device shows the person, and polls with (RFC 8628 section 3.2).
// No `Debug`, so the device code cannot reach a log through one.
#[derive(Serialize, ToSchema)]
pub(crate) struct DeviceAuthorizationResponse {
    /// Secret: the device polls the token endpoint with it. Never shown to the person.
    device_code: String,
    /// What the person types at `verification_uri`, e.g. `BCDF-GHJK`.
    user_code: String,
    /// `{ui_url}/device`.
    verification_uri: String,
    /// `verification_uri` with the user code filled in.
    verification_uri_complete: String,
    /// Seconds until the codes stop working: 600.
    expires_in: i64,
    /// Seconds the device must leave between polls. Polling sooner is answered `slow_down`.
    interval: i32,
}

/// Starts a device authorization. Needs no credential; only the built-in `bia-cli` client may
/// use it, and a request from an address that asked too often is refused.
#[utoipa::path(
    post,
    path = "/oauth/device_authorization",
    tag = "oauth",
    request_body(content = DeviceAuthorizationRequest, content_type = "application/x-www-form-urlencoded"),
    responses(
        (status = OK, body = DeviceAuthorizationResponse, description = "Sent with `Cache-Control: no-store`."),
        (status = BAD_REQUEST, description = "An OAuth error: `invalid_request`, `invalid_client`, `unauthorized_client`, `invalid_scope` or `invalid_target`.", body = OAuthErrorBody),
        (status = SERVICE_UNAVAILABLE, description = "`temporarily_unavailable`: the server has no `ui_url` to send the person to.", body = OAuthErrorBody),
    )
)]
pub(crate) async fn device_authorization(
    State(state): State<ApiState>,
    ClientAddress(address): ClientAddress,
    request: Result<Form<DeviceAuthorizationRequest>, FormRejection>,
) -> Result<Response, OAuthError> {
    let Form(request) = request.map_err(|_| {
        OAuthError::invalid_request("the body must be `application/x-www-form-urlencoded`")
    })?;
    let now = state.clock.now();
    if !state.device_requests.allow(address, now) {
        return Err(OAuthError::unavailable(
            "too many device authorizations from this address; try again later",
        ));
    }
    let Some(client_id) = request.client_id.as_deref().filter(|id| !id.is_empty()) else {
        return Err(OAuthError::invalid_request("`client_id` is required"));
    };
    let client: ResolvedClient = client::resolve(&state, client_id)
        .await
        .map_err(|error| OAuthError::new("invalid_client", error.body().message.clone()))?;
    if client.provenance != Provenance::Builtin {
        return Err(OAuthError::new(
            "unauthorized_client",
            "only the built-in `bia-cli` client may use the device flow",
        ));
    }
    let scope = authorize::scope(request.scope.as_deref())
        .map_err(|description| OAuthError::new("invalid_scope", description))?;
    let resource = request
        .resource
        .as_deref()
        .ok_or_else(|| OAuthError::new("invalid_target", "`resource` is required"))?;
    let Some(resource) = state.config.canonical_resource(resource)? else {
        return Err(OAuthError::new(
            "invalid_target",
            "`resource` is not this server: use its `public_url` or `{public_url}/mcp`",
        ));
    };
    let Some(verification_uri) = state.config.device_verification_url() else {
        return Err(OAuthError::unavailable(
            "bia cannot ask a person to approve a device: it was built without the UI and \
             `ui_url` (BIA_UI_URL) is not set",
        ));
    };

    let (device_code, device_hash) =
        TokenValue::generate_prefixed(DEVICE_CODE_PREFIX).map_err(|e| ApiError::internal(&e))?;
    let user_code = generate_user_code().map_err(|e| ApiError::internal(&e))?;
    let Some(user_hash) = user_code_hash(&user_code) else {
        return Err(ApiError::internal(&std::fmt::Error).into());
    };
    let interval = state.config.device_interval_secs();
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    PgOAuthRepository
        .begin_device_authorization(
            &mut conn,
            &NewDeviceAuthorization {
                client_id: client.client_id,
                client_name: client.name,
                client_host: client.host,
                provenance: client.provenance,
                requested_scope: scope,
                resource,
                device_code: device_hash,
                user_code: user_hash,
                created_at: now,
                expires_at: now + DEVICE_LIFETIME,
                interval_secs: interval,
            },
        )
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let mut response = Json(DeviceAuthorizationResponse {
        device_code: device_code.into_string(),
        verification_uri_complete: format!("{verification_uri}?user_code={user_code}"),
        user_code,
        verification_uri,
        expires_in: DEVICE_LIFETIME.num_seconds(),
        interval,
    })
    .into_response();
    no_store(&mut response);
    Ok(response)
}

/// A device authorization a person is asked about.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DeviceConsentRequest {
    /// Pass it to `GET /oauth/device/{id}` and the approve and deny routes.
    id: Uuid,
    client: ConsentClient,
    /// What the client asked for; the person may approve less.
    requested_scope: GrantScopeName,
    /// What the tokens will be good for: the API, or the MCP endpoint only.
    resource: String,
    expires_at: DateTime<Utc>,
}

impl From<DeviceAuthorization> for DeviceConsentRequest {
    fn from(authorization: DeviceAuthorization) -> Self {
        Self {
            id: authorization.id,
            client: ConsentClient {
                id: authorization.client_id,
                name: authorization.client_name,
                provenance: ProvenanceName::from(authorization.provenance),
                host: authorization.client_host,
                verified: authorization.provenance != Provenance::Dynamic,
            },
            requested_scope: authorization.requested_scope.into(),
            resource: authorization.resource,
            expires_at: authorization.expires_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UserCode {
    /// The code the device shows, in any case, with or without the hyphen.
    user_code: String,
}

fn not_found() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        ErrorCode::DeviceAuthorizationNotFound,
        "no pending device authorization has this code: it was decided already, or it ran out; \
         run `bia login --device` again",
    )
}

/// Finds the pending device authorization a user code names. Attempts are throttled like
/// sign-ins, since a guess that lands would let someone approve their own request. Needs a
/// Session.
#[utoipa::path(
    post,
    path = "/oauth/device/lookup",
    tag = "oauth",
    security(("session" = []), ("token" = [])),
    request_body = UserCode,
    responses(
        (status = OK, body = DeviceConsentRequest),
        (status = NOT_FOUND, description = "`device_authorization_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn lookup(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    ClientAddress(address): ClientAddress,
    Json(request): Json<UserCode>,
) -> Result<Json<DeviceConsentRequest>, ApiError> {
    require_session(&caller)?;
    let subject = state.throttle(caller.user_id, address).await;
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let found = match user_code_hash(&request.user_code) {
        Some(hash) => PgOAuthRepository
            .pending_device_authorization_by_user_code(&mut conn, hash, state.clock.now())
            .await
            .map_err(|e| ApiError::internal(&e))?,
        None => None,
    };
    let pending = found.ok_or_else(not_found)?;
    state.sign_ins.succeeded(subject);
    Ok(Json(pending.into()))
}

/// The pending device authorization with this id. Needs a Session.
#[utoipa::path(
    get,
    path = "/oauth/device/{id}",
    tag = "oauth",
    security(("session" = []), ("token" = [])),
    params(("id" = Uuid, Path, description = "The `id` that `POST /oauth/device/lookup` returned.")),
    responses(
        (status = OK, body = DeviceConsentRequest),
        (status = NOT_FOUND, description = "`device_authorization_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn get_device_consent(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeviceConsentRequest>, ApiError> {
    require_session(&caller)?;
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let pending = PgOAuthRepository
        .pending_device_authorization(&mut conn, id, state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(not_found)?;
    Ok(Json(pending.into()))
}

/// How a device authorization was answered. The device learns the result by polling.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DeviceDecision {
    /// `approved` or `denied`.
    outcome: &'static str,
}

/// Approves the device authorization with the Scope given or less. The device's next poll
/// receives its tokens. Answering twice fails. Needs a Session.
#[utoipa::path(
    post,
    path = "/oauth/device/{id}/approve",
    tag = "oauth",
    security(("session" = []), ("token" = [])),
    params(("id" = Uuid, Path, description = "The `id` that `POST /oauth/device/lookup` returned.")),
    request_body = Approval,
    responses(
        (status = OK, body = DeviceDecision),
        (status = BAD_REQUEST, description = "`scope_exceeds_request`.", body = ErrorBody),
        (status = NOT_FOUND, description = "`device_authorization_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn approve_device(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(approval): Json<Approval>,
) -> Result<Json<DeviceDecision>, ApiError> {
    require_session(&caller)?;
    let now = state.clock.now();
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let pending = PgOAuthRepository
        .pending_device_authorization(&mut conn, id, now)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(not_found)?;
    let scope = approval
        .scope
        .map_or(pending.requested_scope, bia_store::oauth::GrantScope::from);
    if !pending.requested_scope.covers(scope) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::ScopeExceedsRequest,
            "the client asked for less than that; approve what it asked for, or less",
        ));
    }
    let approved = PgOAuthRepository
        .approve_device(&mut conn, id, caller.user_id, scope, now)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(not_found)?;
    tracing::info!(
        device_authorization_id = %approved.id,
        client = %approved.client_name,
        scope = scope.as_str(),
        "device authorization approved"
    );
    Ok(Json(DeviceDecision {
        outcome: "approved",
    }))
}

/// Denies the device authorization; the device's next poll is answered `access_denied`. Needs
/// a Session.
#[utoipa::path(
    post,
    path = "/oauth/device/{id}/deny",
    tag = "oauth",
    security(("session" = []), ("token" = [])),
    params(("id" = Uuid, Path, description = "The `id` that `POST /oauth/device/lookup` returned.")),
    responses(
        (status = OK, body = DeviceDecision),
        (status = NOT_FOUND, description = "`device_authorization_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn deny_device(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeviceDecision>, ApiError> {
    require_session(&caller)?;
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let denied = PgOAuthRepository
        .deny_device(&mut conn, id, state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(not_found)?;
    tracing::info!(device_authorization_id = %denied.id, client = %denied.client_name, "device authorization denied");
    Ok(Json(DeviceDecision { outcome: "denied" }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{generate_user_code, user_code_hash};

    #[test]
    fn a_generated_user_code_is_eight_letters_in_two_groups_and_hashes_the_same_however_typed() {
        let code = generate_user_code().unwrap();

        assert_eq!(code.len(), 9, "{code}");
        assert_eq!(code.as_bytes()[4], b'-');
        let hash = user_code_hash(&code).unwrap();
        assert!(user_code_hash(&code.to_lowercase()).unwrap() == hash);
        assert!(user_code_hash(&code.replace('-', " ")).unwrap() == hash);
        assert!(user_code_hash(&code.replace('-', "")).unwrap() == hash);
    }

    #[test]
    fn what_cannot_be_a_user_code_has_no_hash() {
        for typed in [
            "",
            "ABCD",
            "ABCD-EFG",
            "ABCD-EFGHJ",
            "ABCD-EFG1",
            "AEIO-UAEI",
        ] {
            assert!(user_code_hash(typed).is_none(), "{typed}");
        }
    }

    #[test]
    fn two_user_codes_differ() {
        assert_ne!(generate_user_code().unwrap(), generate_user_code().unwrap());
    }
}
