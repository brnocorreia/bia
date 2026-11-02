//! Passkeys: registered, renamed and removed by a signed-in admin, and used to sign in instead
//! of the password (ADR-0065). Each ceremony is a `begin` that returns the browser's options
//! and keeps the challenge's state server-side, and a `finish` that checks the response.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use bia_store::passkey::{
    CeremonyId, CeremonyKind, NewPasskey, Passkey, PasskeyId, PasskeyRepository,
    PgPasskeyRepository,
};
use bia_store::user::{PgUserRepository, UserId, UserRepository};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;
use webauthn_rs::prelude::{
    PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential,
};

use crate::auth::passkey::{CEREMONY_LIFETIME, REAUTHENTICATION_WINDOW};
use crate::auth::{Admin, AuthErrors, ClientAddress, passkey, session};
use crate::routes::session::SignedIn;
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path};

const MAX_NAME_LEN: usize = 128;

/// A passkey, without its credential.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PasskeyInfo {
    id: Uuid,
    /// What the admin calls it, e.g. `laptop`.
    name: String,
    created_at: DateTime<Utc>,
    /// When it last signed in.
    last_used_at: Option<DateTime<Utc>>,
}

impl From<Passkey> for PasskeyInfo {
    fn from(passkey: Passkey) -> Self {
        Self {
            id: passkey.id.as_uuid(),
            name: passkey.name,
            created_at: passkey.created_at,
            last_used_at: passkey.last_used_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PasskeyList {
    /// The User's passkeys, oldest first.
    passkeys: Vec<PasskeyInfo>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NamePasskey {
    /// What the passkey is called, 1 to 128 characters. Need not be unique.
    #[schema(example = "laptop")]
    name: String,
}

/// A ceremony that has begun.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Ceremony {
    /// Send it back with the browser's response. Valid for 5 minutes, once.
    ceremony_id: Uuid,
    /// The WebAuthn options as JSON, `publicKey` inside: binary fields are base64url strings.
    /// Pass them to `navigator.credentials.create` (registration) or `.get` (sign-in) after
    /// decoding those fields.
    options: serde_json::Value,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinishCeremony {
    ceremony_id: Uuid,
    /// The browser's `PublicKeyCredential` as JSON, with binary fields as base64url strings.
    credential: serde_json::Value,
}

fn trimmed_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Err(ApiError::malformed_request(format!(
            "`name` must be 1 to {MAX_NAME_LEN} characters"
        )));
    }
    Ok(name.to_owned())
}

fn stored<T: Serialize>(value: &T) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(value).map_err(|e| ApiError::internal(&e))
}

fn credentials_of(passkeys: &[Passkey]) -> Vec<webauthn_rs::prelude::Passkey> {
    passkeys
        .iter()
        .filter_map(|passkey| {
            serde_json::from_value(passkey.credential.clone())
                .inspect_err(|error| {
                    tracing::error!(%error, passkey_id = %passkey.id, "a stored passkey cannot be read");
                })
                .ok()
        })
        .collect()
}

fn invalid_ceremony(message: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::InvalidCeremony, message)
}

fn passkey_not_found(id: Uuid) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        ErrorCode::PasskeyNotFound,
        format!("no passkey has id {id}"),
    )
}

async fn acquire(state: &ApiState) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, ApiError> {
    state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))
}

async fn begin_ceremony(
    state: &ApiState,
    conn: &mut PgConnection,
    user: UserId,
    kind: CeremonyKind,
    ceremony_state: &serde_json::Value,
) -> Result<CeremonyId, ApiError> {
    let now = state.clock.now();
    PgPasskeyRepository
        .begin_ceremony(
            conn,
            user,
            kind,
            ceremony_state,
            now,
            now + CEREMONY_LIFETIME,
        )
        .await
        .map_err(|e| ApiError::internal(&e))
}

/// The User's passkeys. Needs an `admin` Token.
#[utoipa::path(
    get,
    path = "/v1/passkeys",
    tag = "passkeys",
    security(("session" = []), ("token" = [])),
    responses((status = OK, body = PasskeyList), AuthErrors)
)]
pub(crate) async fn list_passkeys(
    Admin(caller): Admin,
    State(state): State<ApiState>,
) -> Result<Json<PasskeyList>, ApiError> {
    let mut conn = acquire(&state).await?;
    let passkeys = PgPasskeyRepository
        .list(&mut conn, caller.user_id)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    Ok(Json(PasskeyList {
        passkeys: passkeys.into_iter().map(PasskeyInfo::from).collect(),
    }))
}

/// Begins registering a passkey and returns the options for `navigator.credentials.create`.
/// Needs an `admin` Token.
#[utoipa::path(
    post,
    path = "/v1/passkeys/registration/begin",
    tag = "passkeys",
    security(("session" = []), ("token" = [])),
    request_body = NamePasskey,
    responses(
        (status = OK, body = Ceremony),
        (status = BAD_REQUEST, description = "`malformed_request`: `name` is empty or too long.", body = ErrorBody),
        (status = CONFLICT, description = "`passkeys_unavailable`: `public_url` cannot be a relying party.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn begin_registration(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Json(request): Json<NamePasskey>,
) -> Result<Json<Ceremony>, ApiError> {
    let name = trimmed_name(&request.name)?;
    let webauthn = passkey::webauthn(&state.config)?;
    let mut conn = acquire(&state).await?;
    let existing = PgPasskeyRepository
        .list(&mut conn, caller.user_id)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let exclude: Vec<_> = credentials_of(&existing)
        .iter()
        .map(|credential| credential.cred_id().clone())
        .collect();
    let (options, registration) = webauthn
        .start_passkey_registration(
            caller.user_id.as_uuid(),
            "admin",
            "bia admin",
            Some(exclude).filter(|exclude| !exclude.is_empty()),
        )
        .map_err(|e| ApiError::internal(&e))?;
    let ceremony = begin_ceremony(
        &state,
        &mut conn,
        caller.user_id,
        CeremonyKind::Registration,
        &serde_json::json!({ "name": name, "registration": stored(&registration)? }),
    )
    .await?;
    Ok(Json(Ceremony {
        ceremony_id: ceremony.as_uuid(),
        options: stored(&options)?,
    }))
}

/// Finishes registering a passkey with the browser's response. The response must come from
/// `public_url`'s origin and RP ID. Needs an `admin` Token.
#[utoipa::path(
    post,
    path = "/v1/passkeys/registration/finish",
    tag = "passkeys",
    security(("session" = []), ("token" = [])),
    request_body = FinishCeremony,
    responses(
        (status = CREATED, body = PasskeyInfo),
        (status = BAD_REQUEST, description = "`invalid_ceremony`: unknown, expired or used ceremony, or a response that does not verify, e.g. from another origin.", body = ErrorBody),
        (status = CONFLICT, description = "`passkeys_unavailable`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn finish_registration(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Json(request): Json<FinishCeremony>,
) -> Result<(StatusCode, Json<PasskeyInfo>), ApiError> {
    let webauthn = passkey::webauthn(&state.config)?;
    let mut conn = acquire(&state).await?;
    let taken = PgPasskeyRepository
        .take_ceremony(
            &mut conn,
            caller.user_id,
            CeremonyId::from_uuid(request.ceremony_id),
            CeremonyKind::Registration,
            state.clock.now(),
        )
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(|| invalid_ceremony("the ceremony is unknown, expired or already used"))?;
    #[derive(Deserialize)]
    struct Begun {
        name: String,
        registration: PasskeyRegistration,
    }
    let begun: Begun = serde_json::from_value(taken).map_err(|e| ApiError::internal(&e))?;
    let response: RegisterPublicKeyCredential = serde_json::from_value(request.credential)
        .map_err(|_| invalid_ceremony("`credential` is not a registration response"))?;
    let credential = webauthn
        .finish_passkey_registration(&response, &begun.registration)
        .map_err(|error| {
            tracing::warn!(%error, "passkey registration refused");
            invalid_ceremony("the authenticator's response was refused")
        })?;
    let existing = PgPasskeyRepository
        .list(&mut conn, caller.user_id)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    if credentials_of(&existing)
        .iter()
        .any(|known| known.cred_id() == credential.cred_id())
    {
        return Err(invalid_ceremony("this authenticator is already registered"));
    }
    let created = PgPasskeyRepository
        .create(
            &mut conn,
            &NewPasskey {
                user_id: caller.user_id,
                name: begun.name,
                credential: stored(&credential)?,
                created_at: state.clock.now(),
            },
        )
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tracing::info!(user_id = %caller.user_id, passkey_id = %created.id, "passkey registered");
    Ok((StatusCode::CREATED, Json(created.into())))
}

/// Renames a passkey. Needs an `admin` Token.
#[utoipa::path(
    patch,
    path = "/v1/passkeys/{passkey_id}",
    tag = "passkeys",
    security(("session" = []), ("token" = [])),
    params(("passkey_id" = Uuid, Path, description = "The passkey's `id`.")),
    request_body = NamePasskey,
    responses(
        (status = OK, body = PasskeyInfo),
        (status = BAD_REQUEST, description = "`malformed_request`: `name` is empty or too long.", body = ErrorBody),
        (status = NOT_FOUND, description = "`passkey_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn rename_passkey(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Path(passkey_id): Path<Uuid>,
    Json(request): Json<NamePasskey>,
) -> Result<Json<PasskeyInfo>, ApiError> {
    let name = trimmed_name(&request.name)?;
    let mut conn = acquire(&state).await?;
    let renamed = PgPasskeyRepository
        .rename(
            &mut conn,
            caller.user_id,
            PasskeyId::from_uuid(passkey_id),
            &name,
        )
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(|| passkey_not_found(passkey_id))?;
    Ok(Json(renamed.into()))
}

/// Removes a passkey. The caller must be a Session that signed in within the last 10 minutes,
/// which a Token that is not a Session cannot prove.
#[utoipa::path(
    delete,
    path = "/v1/passkeys/{passkey_id}",
    tag = "passkeys",
    security(("session" = []), ("token" = [])),
    params(("passkey_id" = Uuid, Path, description = "The passkey's `id`.")),
    responses(
        (status = NO_CONTENT, description = "The passkey is gone."),
        (status = FORBIDDEN, description = "`reauthentication_required`: sign in again, then retry.", body = ErrorBody),
        (status = NOT_FOUND, description = "`passkey_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn delete_passkey(
    Admin(caller): Admin,
    State(state): State<ApiState>,
    Path(passkey_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let signed_in_recently = caller.origin == bia_store::token::Origin::Session
        && state.clock.now() - caller.created_at < REAUTHENTICATION_WINDOW;
    if !signed_in_recently {
        return Err(passkey::reauthentication_required());
    }
    let mut conn = acquire(&state).await?;
    let deleted = PgPasskeyRepository
        .delete(&mut conn, caller.user_id, PasskeyId::from_uuid(passkey_id))
        .await
        .map_err(|e| ApiError::internal(&e))?;
    if !deleted {
        return Err(passkey_not_found(passkey_id));
    }
    tracing::warn!(user_id = %caller.user_id, %passkey_id, "passkey removed");
    Ok(StatusCode::NO_CONTENT)
}

/// Begins signing in with a passkey and returns the options for `navigator.credentials.get`.
/// Public, like the password sign-in.
#[utoipa::path(
    post,
    path = "/v1/session/passkey/begin",
    tag = "session",
    responses(
        (status = OK, body = Ceremony),
        (status = UNAUTHORIZED, description = "`invalid_credentials`: no passkey is registered.", body = ErrorBody),
        (status = CONFLICT, description = "`passkeys_unavailable`: `public_url` cannot be a relying party.", body = ErrorBody),
    )
)]
pub(crate) async fn begin_sign_in(
    State(state): State<ApiState>,
) -> Result<Json<Ceremony>, ApiError> {
    let webauthn = passkey::webauthn(&state.config)?;
    let mut conn = acquire(&state).await?;
    let user = PgUserRepository
        .credentials(&mut conn)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(no_passkey)?
        .user;
    let stored_passkeys = PgPasskeyRepository
        .list(&mut conn, user.id)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let credentials = credentials_of(&stored_passkeys);
    if credentials.is_empty() {
        return Err(no_passkey());
    }
    let (options, authentication) = webauthn
        .start_passkey_authentication(&credentials)
        .map_err(|e| ApiError::internal(&e))?;
    let ceremony = begin_ceremony(
        &state,
        &mut conn,
        user.id,
        CeremonyKind::Authentication,
        &stored(&authentication)?,
    )
    .await?;
    Ok(Json(Ceremony {
        ceremony_id: ceremony.as_uuid(),
        options: stored(&options)?,
    }))
}

fn no_passkey() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::InvalidCredentials,
        "no passkey is registered",
    )
}

fn passkey_refused() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::InvalidCredentials,
        "the passkey was refused",
    )
}

/// Finishes signing in with the browser's response and starts the same Session as the password
/// does, with the same cookie. A response that fails, for whatever reason, counts towards
/// sign-in throttling like a wrong password. Public.
#[utoipa::path(
    post,
    path = "/v1/session/passkey/finish",
    tag = "session",
    request_body = FinishCeremony,
    responses(
        (status = CREATED, body = SignedIn, headers(("Set-Cookie" = String, description = "`bia_session=<value>`"))),
        (status = UNAUTHORIZED, description = "`invalid_credentials`: unknown, expired or used ceremony, or a response that does not verify, e.g. from another origin.", body = ErrorBody),
        (status = CONFLICT, description = "`passkeys_unavailable`.", body = ErrorBody),
    )
)]
pub(crate) async fn finish_sign_in(
    State(state): State<ApiState>,
    ClientAddress(address): ClientAddress,
    headers: HeaderMap,
    Json(request): Json<FinishCeremony>,
) -> Result<
    (
        StatusCode,
        [(header::HeaderName, HeaderValue); 1],
        Json<SignedIn>,
    ),
    ApiError,
> {
    let webauthn = passkey::webauthn(&state.config)?;
    let mut conn = acquire(&state).await?;
    let user = PgUserRepository
        .credentials(&mut conn)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(passkey_refused)?
        .user;
    let subject = state.throttle(user.id, address).await;
    let taken = PgPasskeyRepository
        .take_ceremony(
            &mut conn,
            user.id,
            CeremonyId::from_uuid(request.ceremony_id),
            CeremonyKind::Authentication,
            state.clock.now(),
        )
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(passkey_refused)?;
    let authentication: PasskeyAuthentication =
        serde_json::from_value(taken).map_err(|e| ApiError::internal(&e))?;
    let response: PublicKeyCredential =
        serde_json::from_value(request.credential).map_err(|_| passkey_refused())?;
    let result = webauthn
        .finish_passkey_authentication(&response, &authentication)
        .map_err(|error| {
            tracing::warn!(%error, "passkey sign-in refused");
            passkey_refused()
        })?;
    let stored_passkeys = PgPasskeyRepository
        .list(&mut conn, user.id)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let (used, mut credential) = stored_passkeys
        .iter()
        .filter_map(|passkey| {
            let credential: webauthn_rs::prelude::Passkey =
                serde_json::from_value(passkey.credential.clone()).ok()?;
            (credential.cred_id() == result.cred_id()).then_some((passkey, credential))
        })
        .next()
        .ok_or_else(passkey_refused)?;
    credential.update_credential(&result);
    PgPasskeyRepository
        .record_use(&mut conn, used.id, &stored(&credential)?, state.clock.now())
        .await
        .map_err(|e| ApiError::internal(&e))?;
    state.sign_ins.succeeded(subject);
    let (token, value) =
        session::start(&state, &mut conn, user.id, session::user_agent(&headers)).await?;
    tracing::info!(user_id = %user.id, session_id = %token.id, passkey_id = %used.id, "signed in with a passkey");
    Ok((
        StatusCode::CREATED,
        [(
            header::SET_COOKIE,
            session::set_cookie(&value, state.config.secure_cookies()),
        )],
        Json(SignedIn {
            user: user.into(),
            session: token.into(),
        }),
    ))
}
