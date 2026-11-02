//! `POST /oauth/register`: Dynamic Client Registration (RFC 7591), kept for clients that cannot
//! use a Client ID Metadata Document. Anyone may register, so the client is marked unverified,
//! its name is only a claim, and registrations are rate-limited in memory.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bia_store::oauth::{NewClient, OAuthRepository, PgOAuthRepository};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::client::{display_name, validate_redirect_uri};
use super::error::no_store;
use super::{OAuthError, OAuthErrorBody};
use crate::ApiState;
use crate::auth::{ClientAddress, TokenValue};

const MAX_REDIRECT_URIS: usize = 10;
const MAX_REDIRECT_URI_LEN: usize = 2048;
/// Registrations one address may make per window.
const REGISTRATIONS_PER_WINDOW: u32 = 20;
const WINDOW: TimeDelta = TimeDelta::hours(1);
/// Addresses remembered at once; beyond it, the oldest windows are forgotten first.
const MAX_ADDRESSES: usize = 10_000;

/// Registrations per address in a window, counted in memory: a restart or another API process
/// starts from zero. Behind a proxy the address is the proxy's, so all callers share one count.
#[derive(Debug, Default)]
pub(crate) struct Limiter(Mutex<HashMap<IpAddr, (DateTime<Utc>, u32)>>);

impl Limiter {
    /// Counts a registration from `address`; `false` when it has used up its window.
    pub(crate) fn allow(&self, address: IpAddr, now: DateTime<Utc>) -> bool {
        let mut windows = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        windows.retain(|_, (started, _)| now - *started < WINDOW);
        if windows.len() >= MAX_ADDRESSES && !windows.contains_key(&address) {
            return false;
        }
        let (_, count) = windows.entry(address).or_insert((now, 0));
        if *count >= REGISTRATIONS_PER_WINDOW {
            return false;
        }
        *count += 1;
        true
    }
}

/// Client metadata (RFC 7591). Other members are ignored.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct Registration {
    /// Where the client may be sent back to: `https`, `http` on loopback (any port), or a
    /// native app's own scheme. One to ten, exactly as the client will send them.
    redirect_uris: Vec<String>,
    /// Shown to the person when they approve; unverified, since anyone can claim any name.
    client_name: Option<String>,
    /// Only `none`: clients are public and use PKCE.
    token_endpoint_auth_method: Option<String>,
}

/// A registered client.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Registered {
    /// Use it as `client_id` in authorization and token requests.
    client_id: String,
    client_name: String,
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: &'static str,
    grant_types: Vec<&'static str>,
    response_types: Vec<&'static str>,
    client_id_issued_at: i64,
}

/// Registers a client. Open to anyone, since a client has to register before it can ask; the
/// person who approves it is told it registered itself and is unverified. At most 20 per
/// address per hour.
#[utoipa::path(
    post,
    path = "/oauth/register",
    tag = "oauth",
    request_body = Registration,
    responses(
        (status = CREATED, body = Registered),
        (status = BAD_REQUEST, description = "`invalid_redirect_uri` or `invalid_client_metadata`.", body = OAuthErrorBody),
        (status = TOO_MANY_REQUESTS, description = "`rate_limited`.", body = OAuthErrorBody),
    )
)]
pub(crate) async fn register(
    State(state): State<ApiState>,
    ClientAddress(address): ClientAddress,
    request: Result<axum::Json<Registration>, JsonRejection>,
) -> Result<Response, OAuthError> {
    let now = state.clock.now();
    if !state.registrations.allow(address, now) {
        let mut error = OAuthError::new("rate_limited", "too many registrations; try again later")
            .into_response();
        *error.status_mut() = StatusCode::TOO_MANY_REQUESTS;
        error
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(WINDOW.num_seconds()));
        return Ok(error);
    }
    let axum::Json(request) = request.map_err(|_| {
        OAuthError::new(
            "invalid_client_metadata",
            "the body must be a JSON object with `redirect_uris`",
        )
    })?;
    if request
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|method| method != "none")
    {
        return Err(OAuthError::new(
            "invalid_client_metadata",
            "`token_endpoint_auth_method` must be `none`",
        ));
    }
    if request.redirect_uris.is_empty() || request.redirect_uris.len() > MAX_REDIRECT_URIS {
        return Err(OAuthError::new(
            "invalid_redirect_uri",
            format!("`redirect_uris` must list 1 to {MAX_REDIRECT_URIS} URIs"),
        ));
    }
    for uri in &request.redirect_uris {
        if uri.len() > MAX_REDIRECT_URI_LEN || validate_redirect_uri(uri).is_err() {
            return Err(OAuthError::new(
                "invalid_redirect_uri",
                "a redirect URI must be `https`, `http` on loopback or a native app scheme, \
                 without a fragment or credentials",
            ));
        }
    }
    let (id, _) =
        TokenValue::generate_prefixed("bia_client_").map_err(|e| crate::ApiError::internal(&e))?;
    let client = NewClient {
        client_id: id.into_string(),
        client_name: display_name(request.client_name.as_deref())
            .unwrap_or_else(|| "Unnamed client".to_owned()),
        redirect_uris: request.redirect_uris,
        created_at: now,
    };
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    PgOAuthRepository
        .register_client(&mut conn, &client)
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    tracing::info!(client = %client.client_name, "client registered itself");
    let mut response = (
        StatusCode::CREATED,
        axum::Json(Registered {
            client_id: client.client_id,
            client_name: client.client_name,
            redirect_uris: client.redirect_uris,
            token_endpoint_auth_method: "none",
            grant_types: vec!["authorization_code", "refresh_token"],
            response_types: vec!["code"],
            client_id_issued_at: now.timestamp(),
        }),
    )
        .into_response();
    no_store(&mut response);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use chrono::{DateTime, TimeDelta};

    use super::{Limiter, REGISTRATIONS_PER_WINDOW};

    #[test]
    fn an_address_gets_its_registrations_per_window_and_a_new_window_after() {
        let limiter = Limiter::default();
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap_or_default();
        let here: IpAddr = [10, 0, 0, 1].into();
        let there: IpAddr = [10, 0, 0, 2].into();

        for _ in 0..REGISTRATIONS_PER_WINDOW {
            assert!(limiter.allow(here, now));
        }
        assert!(!limiter.allow(here, now));
        assert!(limiter.allow(there, now));
        assert!(limiter.allow(here, now + TimeDelta::hours(1)));
    }
}
