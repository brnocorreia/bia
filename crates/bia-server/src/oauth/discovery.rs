//! Discovery: what a client reads to find the authorization server (RFC 9728, RFC 8414).

use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;

use crate::{ApiError, ApiState, Json};

/// OAuth 2.0 Protected Resource Metadata (RFC 9728).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ProtectedResourceMetadata {
    /// The resource these tokens are for: the MCP endpoint, or the whole API.
    resource: String,
    /// The one authorization server: this one.
    authorization_servers: Vec<String>,
    /// `agent`: what an MCP client needs; `admin` is only granted if the person approves it.
    scopes_supported: Vec<&'static str>,
    bearer_methods_supported: Vec<&'static str>,
}

/// OAuth 2.0 Authorization Server Metadata (RFC 8414).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    revocation_endpoint: String,
    registration_endpoint: String,
    /// The device authorization grant (RFC 8628), which only the built-in `bia-cli` client may
    /// use.
    device_authorization_endpoint: String,
    scopes_supported: Vec<&'static str>,
    response_types_supported: Vec<&'static str>,
    grant_types_supported: Vec<&'static str>,
    /// `S256` only.
    code_challenge_methods_supported: Vec<&'static str>,
    /// `none`: clients are public and prove themselves with PKCE.
    token_endpoint_auth_methods_supported: Vec<&'static str>,
    revocation_endpoint_auth_methods_supported: Vec<&'static str>,
    /// `true`: every authorization response, errors included, carries `iss` (RFC 9207).
    authorization_response_iss_parameter_supported: bool,
    /// `true`: a `client_id` that is an `https` URL names a Client ID Metadata Document.
    client_id_metadata_document_supported: bool,
}

/// The MCP endpoint's metadata: which authorization server issues tokens for `/mcp`. Public.
#[utoipa::path(
    get,
    path = "/.well-known/oauth-protected-resource/mcp",
    tag = "oauth",
    responses(
        (status = OK, body = ProtectedResourceMetadata),
        (status = SERVICE_UNAVAILABLE, description = "`public_url_missing`.", body = crate::ErrorBody),
    )
)]
pub(crate) async fn mcp_resource_metadata(
    State(state): State<ApiState>,
) -> Result<Json<ProtectedResourceMetadata>, ApiError> {
    resource_metadata(&state, state.config.mcp_resource()?)
}

/// The API's metadata, for tokens issued for the whole API. Public.
#[utoipa::path(
    get,
    path = "/.well-known/oauth-protected-resource",
    tag = "oauth",
    responses(
        (status = OK, body = ProtectedResourceMetadata),
        (status = SERVICE_UNAVAILABLE, description = "`public_url_missing`.", body = crate::ErrorBody),
    )
)]
pub(crate) async fn api_resource_metadata(
    State(state): State<ApiState>,
) -> Result<Json<ProtectedResourceMetadata>, ApiError> {
    resource_metadata(&state, state.config.issuer()?)
}

fn resource_metadata(
    state: &ApiState,
    resource: String,
) -> Result<Json<ProtectedResourceMetadata>, ApiError> {
    Ok(Json(ProtectedResourceMetadata {
        resource,
        authorization_servers: vec![state.config.issuer()?],
        scopes_supported: vec!["agent"],
        bearer_methods_supported: vec!["header"],
    }))
}

/// The authorization server's endpoints and what it supports. Public.
#[utoipa::path(
    get,
    path = "/.well-known/oauth-authorization-server",
    tag = "oauth",
    responses(
        (status = OK, body = AuthorizationServerMetadata),
        (status = SERVICE_UNAVAILABLE, description = "`public_url_missing`.", body = crate::ErrorBody),
    )
)]
pub(crate) async fn authorization_server_metadata(
    State(state): State<ApiState>,
) -> Result<Json<AuthorizationServerMetadata>, ApiError> {
    let issuer = state.config.issuer()?;
    Ok(Json(AuthorizationServerMetadata {
        authorization_endpoint: format!("{issuer}/oauth/authorize"),
        token_endpoint: format!("{issuer}/oauth/token"),
        revocation_endpoint: format!("{issuer}/oauth/revoke"),
        registration_endpoint: format!("{issuer}/oauth/register"),
        device_authorization_endpoint: format!("{issuer}/oauth/device_authorization"),
        issuer,
        scopes_supported: vec!["agent", "admin"],
        response_types_supported: vec!["code"],
        grant_types_supported: vec![
            "authorization_code",
            "refresh_token",
            super::device::GRANT_TYPE,
        ],
        code_challenge_methods_supported: vec!["S256"],
        token_endpoint_auth_methods_supported: vec!["none"],
        revocation_endpoint_auth_methods_supported: vec!["none"],
        authorization_response_iss_parameter_supported: true,
        client_id_metadata_document_supported: true,
    }))
}
