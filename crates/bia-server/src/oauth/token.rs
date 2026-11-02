//! `POST /oauth/token`: exchanges an authorization code or a refresh token for tokens.
//!
//! Clients are public and prove themselves with PKCE, so there is no client secret. A code is
//! spent when it is first presented, whatever else is wrong with the request, and presenting a
//! spent code revokes the Grant it produced. A refresh token is replaced each time it is used,
//! and a replaced one that comes back revokes its Grant (ADR-0067).

use axum::extract::State;
use axum::extract::rejection::FormRejection;
use axum::response::{IntoResponse, Response};
use axum::{Form, Json};
use bia_store::oauth::{
    CodeRedemption, DevicePoll, GrantScope, NewGrant, OAuthRepository, PgOAuthRepository,
    RefreshRedemption,
};
use serde::Deserialize;
use sqlx::Acquire;
use utoipa::ToSchema;

use super::device;
use super::error::no_store;
use super::issue::{IssuedTokens, issue_tokens};
use super::{OAuthError, OAuthErrorBody, pkce};
use crate::ApiState;
use crate::auth::secret_hash;

/// The form a client posts. Each grant type needs its own fields.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct TokenRequest {
    /// `authorization_code`, `refresh_token` or, for the built-in `bia-cli` client,
    /// `urn:ietf:params:oauth:grant-type:device_code`.
    grant_type: Option<String>,
    /// The client's id, as in the authorization request.
    client_id: Option<String>,
    /// `authorization_code`: the code the redirect carried.
    code: Option<String>,
    /// `authorization_code`: the `redirect_uri` of the authorization request, exactly.
    redirect_uri: Option<String>,
    /// `authorization_code`: the PKCE verifier whose SHA-256 is the `code_challenge`.
    code_verifier: Option<String>,
    /// `refresh_token`: the refresh token. It is spent by this request.
    refresh_token: Option<String>,
    /// `urn:ietf:params:oauth:grant-type:device_code`: the `device_code` that
    /// `POST /oauth/device_authorization` returned. Polling before the `interval` is answered
    /// `slow_down`; `authorization_pending` means keep polling; `expired_token` and
    /// `access_denied` end the flow. It is spent when it returns tokens.
    device_code: Option<String>,
    /// The `resource` of the authorization request; refused if it is a different one.
    resource: Option<String>,
    /// `refresh_token`: a Scope no broader than the Grant's.
    scope: Option<String>,
}

/// Issues an access token and a refresh token. Needs no credential: the code, its PKCE
/// verifier and the client id are the proof.
#[utoipa::path(
    post,
    path = "/oauth/token",
    tag = "oauth",
    request_body(content = TokenRequest, content_type = "application/x-www-form-urlencoded"),
    responses(
        (status = OK, body = IssuedTokens, description = "Sent with `Cache-Control: no-store`."),
        (status = BAD_REQUEST, description = "An OAuth error: `invalid_request`, `invalid_grant`, `invalid_scope`, `invalid_target` or `unsupported_grant_type`.", body = OAuthErrorBody),
    )
)]
pub(crate) async fn token(
    State(state): State<ApiState>,
    request: Result<Form<TokenRequest>, FormRejection>,
) -> Result<Response, OAuthError> {
    let Form(request) = request.map_err(|_| {
        OAuthError::invalid_request(
            "the body must be `application/x-www-form-urlencoded` with the parameters of the \
             grant type",
        )
    })?;
    let issued = match request.grant_type.as_deref() {
        Some("authorization_code") => exchange_code(&state, &request).await?,
        Some("refresh_token") => exchange_refresh_token(&state, &request).await?,
        Some(device::GRANT_TYPE) => exchange_device_code(&state, &request).await?,
        _ => {
            return Err(OAuthError::new(
                "unsupported_grant_type",
                "`grant_type` must be `authorization_code`, `refresh_token` or the device code grant",
            ));
        }
    };
    let mut response = Json(issued).into_response();
    no_store(&mut response);
    Ok(response)
}

fn required<'a>(value: &'a Option<String>, name: &str) -> Result<&'a str, OAuthError> {
    value
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OAuthError::invalid_request(format!("`{name}` is required")))
}

async fn exchange_code(
    state: &ApiState,
    request: &TokenRequest,
) -> Result<IssuedTokens, OAuthError> {
    let code = required(&request.code, "code")?;
    let client_id = required(&request.client_id, "client_id")?;
    let redirect_uri = required(&request.redirect_uri, "redirect_uri")?;
    let verifier = required(&request.code_verifier, "code_verifier")?;
    if !pkce::is_verifier(verifier) {
        return Err(OAuthError::invalid_request(
            "`code_verifier` must be 43 to 128 characters of `A-Z a-z 0-9 - . _ ~`",
        ));
    }
    let now = state.clock.now();
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    // The code is spent in its own transaction, so it stays spent when a check below fails.
    let redemption = PgOAuthRepository
        .redeem_code(&mut conn, secret_hash(code), now)
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    let authorization = match redemption {
        CodeRedemption::Redeemed(authorization) => authorization,
        CodeRedemption::Replayed => {
            tracing::warn!("authorization code used twice; its Grant is revoked");
            return Err(OAuthError::invalid_grant("the code was already used"));
        }
        CodeRedemption::Unknown => {
            return Err(OAuthError::invalid_grant(
                "the code is unknown or has expired",
            ));
        }
    };
    if authorization.client_id != client_id || authorization.redirect_uri != redirect_uri {
        return Err(OAuthError::invalid_grant(
            "the code was not issued to this `client_id` and `redirect_uri`",
        ));
    }
    if !pkce::verifies(verifier, &authorization.code_challenge) {
        return Err(OAuthError::invalid_grant(
            "`code_verifier` does not match the `code_challenge`",
        ));
    }
    if let Some(resource) = request.resource.as_deref()
        && state.config.canonical_resource(resource)?.as_deref() != Some(&authorization.resource)
    {
        return Err(OAuthError::new(
            "invalid_target",
            "`resource` is not the one the code was issued for",
        ));
    }
    let (Some(user_id), Some(scope)) = (authorization.user_id, authorization.granted_scope) else {
        return Err(OAuthError::invalid_grant("the code was never approved"));
    };
    let mut tx = conn
        .begin()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    let grant = PgOAuthRepository
        .grant_from_authorization(
            &mut tx,
            authorization.id,
            &NewGrant {
                user_id,
                client_id: authorization.client_id,
                client_name: authorization.client_name,
                client_host: authorization.client_host,
                provenance: authorization.provenance,
                scope,
                resource: authorization.resource,
                created_at: now,
            },
        )
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    let issued = issue_tokens(state, &mut tx, &grant, scope).await?;
    tx.commit()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    tracing::info!(grant_id = %grant.id, client = %grant.client_name, scope = scope.as_str(), "grant created");
    Ok(issued)
}

async fn exchange_refresh_token(
    state: &ApiState,
    request: &TokenRequest,
) -> Result<IssuedTokens, OAuthError> {
    let refresh_token = required(&request.refresh_token, "refresh_token")?;
    let client_id = required(&request.client_id, "client_id")?;
    let now = state.clock.now();
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    let mut tx = conn
        .begin()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    let redemption = PgOAuthRepository
        .redeem_refresh_token(&mut tx, secret_hash(refresh_token), now)
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    // The refresh token is spent, and a reused one has revoked its Grant, so the transaction
    // commits whether or not the rest of the request is acceptable.
    let outcome = match redemption {
        RefreshRedemption::Redeemed(grant) => {
            refresh(state, &mut tx, request, client_id, &grant).await
        }
        RefreshRedemption::Reused(grant) => {
            tracing::warn!(grant_id = %grant.id, client = %grant.client_name, "refresh token reused; Grant revoked");
            Err(OAuthError::invalid_grant(
                "the refresh token was already used; the Grant is revoked, so authorize again",
            ))
        }
        RefreshRedemption::Revoked => Err(OAuthError::invalid_grant("the Grant is revoked")),
        RefreshRedemption::Expired => {
            Err(OAuthError::invalid_grant("the refresh token has expired"))
        }
        RefreshRedemption::Unknown => {
            Err(OAuthError::invalid_grant("the refresh token is unknown"))
        }
    };
    tx.commit()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    outcome
}

async fn refresh(
    state: &ApiState,
    conn: &mut sqlx::PgConnection,
    request: &TokenRequest,
    client_id: &str,
    grant: &bia_store::oauth::Grant,
) -> Result<IssuedTokens, OAuthError> {
    if grant.client_id != client_id {
        return Err(OAuthError::invalid_grant(
            "the refresh token was not issued to this `client_id`",
        ));
    }
    if let Some(resource) = request.resource.as_deref()
        && state.config.canonical_resource(resource)?.as_deref() != Some(&grant.resource)
    {
        return Err(OAuthError::new(
            "invalid_target",
            "`resource` is not the one the Grant was made for",
        ));
    }
    let mut names = request
        .scope
        .as_deref()
        .unwrap_or_default()
        .split_whitespace();
    if names
        .clone()
        .any(|name| !matches!(name, "admin" | "agent" | "offline_access"))
    {
        return Err(OAuthError::new(
            "invalid_scope",
            "`scope` may name `agent` and `admin`",
        ));
    }
    let scope = if names.any(|name| name == "admin") {
        GrantScope::Admin
    } else if request
        .scope
        .as_deref()
        .is_some_and(|s| s.split_whitespace().any(|n| n == "agent"))
    {
        GrantScope::Agent
    } else {
        grant.scope
    };
    if !grant.scope.covers(scope) {
        return Err(OAuthError::new(
            "invalid_scope",
            "`scope` may not be broader than the Grant's",
        ));
    }
    Ok(issue_tokens(state, conn, grant, scope).await?)
}

async fn exchange_device_code(
    state: &ApiState,
    request: &TokenRequest,
) -> Result<IssuedTokens, OAuthError> {
    let device_code = required(&request.device_code, "device_code")?;
    let client_id = required(&request.client_id, "client_id")?;
    let now = state.clock.now();
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    // One transaction, so an approval is only spent if the Grant and its tokens are made.
    let mut tx = conn
        .begin()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    let poll = PgOAuthRepository
        .poll_device(&mut tx, secret_hash(device_code), now)
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    let authorization = match poll {
        DevicePoll::Approved(authorization) => authorization,
        other => {
            // A poll that was too fast has recorded the new interval, so this commits too.
            tx.commit()
                .await
                .map_err(|e| crate::ApiError::internal(&e))?;
            return Err(match other {
                DevicePoll::Pending => OAuthError::new(
                    "authorization_pending",
                    "the person has not answered yet; keep polling at the interval",
                ),
                DevicePoll::SlowDown { interval_secs } => OAuthError::new(
                    "slow_down",
                    format!("polling too fast; wait {interval_secs} seconds between polls"),
                ),
                DevicePoll::Expired => {
                    OAuthError::new("expired_token", "the device code has expired; start again")
                }
                DevicePoll::Denied => {
                    OAuthError::new("access_denied", "the person denied the request")
                }
                DevicePoll::Used => {
                    tracing::warn!("device code used twice; its Grant is revoked");
                    OAuthError::invalid_grant("the device code was already used")
                }
                DevicePoll::Unknown | DevicePoll::Approved(_) => {
                    OAuthError::invalid_grant("the device code is unknown")
                }
            });
        }
    };
    if authorization.client_id != client_id {
        tx.commit()
            .await
            .map_err(|e| crate::ApiError::internal(&e))?;
        return Err(OAuthError::invalid_grant(
            "the device code was not issued to this `client_id`",
        ));
    }
    if let Some(resource) = request.resource.as_deref()
        && state.config.canonical_resource(resource)?.as_deref() != Some(&authorization.resource)
    {
        tx.commit()
            .await
            .map_err(|e| crate::ApiError::internal(&e))?;
        return Err(OAuthError::new(
            "invalid_target",
            "`resource` is not the one the device code was issued for",
        ));
    }
    let (Some(user_id), Some(scope)) = (authorization.user_id, authorization.granted_scope) else {
        return Err(OAuthError::invalid_grant(
            "the device code was never approved",
        ));
    };
    let grant = PgOAuthRepository
        .create_grant(
            &mut tx,
            &NewGrant {
                user_id,
                client_id: authorization.client_id,
                client_name: authorization.client_name,
                client_host: authorization.client_host,
                provenance: authorization.provenance,
                scope,
                resource: authorization.resource,
                created_at: now,
            },
        )
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    PgOAuthRepository
        .link_device_grant(&mut tx, authorization.id, grant.id)
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    let issued = issue_tokens(state, &mut tx, &grant, scope).await?;
    tx.commit()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    tracing::info!(grant_id = %grant.id, client = %grant.client_name, scope = scope.as_str(), "grant created from a device code");
    Ok(issued)
}
