//! The OAuth calls `bia login` makes to the server it signs in to (ADR-0068): discovery, the
//! token endpoint's grants, revocation and the device authorization request.
//!
//! Nothing here prints or logs a value it receives; errors carry the server's `error` code and
//! description, which hold no credential.

use std::fmt;

use anyhow::{Context, anyhow};
use serde::Deserialize;

/// The `client_id` the server knows the CLI by without registration.
pub(crate) const CLIENT_ID: &str = "bia-cli";
pub(crate) const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Debug, Deserialize)]
pub(crate) struct Metadata {
    pub(crate) issuer: String,
    pub(crate) authorization_endpoint: String,
    pub(crate) token_endpoint: String,
    pub(crate) revocation_endpoint: String,
    pub(crate) device_authorization_endpoint: String,
}

/// The server's endpoints and the `issuer` that names it: the `resource` its tokens are for.
pub(crate) async fn discover(http: &reqwest::Client, url: &str) -> anyhow::Result<Metadata> {
    let endpoint = format!(
        "{}/.well-known/oauth-authorization-server",
        url.trim_end_matches('/')
    );
    let response = http
        .get(&endpoint)
        .send()
        .await
        .with_context(|| format!("cannot reach the server at {url}"))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "{url} does not offer sign-in: {endpoint} answered {}; the server needs \
             `public_url` (BIA_PUBLIC_URL) set, and an older server cannot be signed in to",
            response.status()
        ));
    }
    response
        .json()
        .await
        .with_context(|| format!("{endpoint} did not answer with OAuth metadata"))
}

#[derive(Deserialize)]
pub(crate) struct Tokens {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) expires_in: i64,
    #[serde(default)]
    pub(crate) scope: String,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

/// Why the token endpoint did not issue tokens.
#[derive(Debug)]
pub(crate) enum TokenError {
    /// The server answered with an OAuth error.
    Oauth { error: String, description: String },
    /// No usable answer: unreachable, or not an OAuth error body.
    Transport(String),
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oauth { error, description } if description.is_empty() => write!(f, "{error}"),
            Self::Oauth { error, description } => write!(f, "{error}: {description}"),
            Self::Transport(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for TokenError {}

#[derive(Deserialize)]
struct OauthErrorBody {
    error: String,
    #[serde(default)]
    error_description: String,
}

async fn post_form<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    endpoint: &str,
    form: &[(&str, &str)],
) -> Result<T, TokenError> {
    let response = http
        .post(endpoint)
        .form(form)
        .send()
        .await
        .map_err(|e| TokenError::Transport(format!("cannot reach {endpoint}: {e}")))?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|e| TokenError::Transport(format!("cannot read the answer of {endpoint}: {e}")))?;
    if status.is_success() {
        return serde_json::from_slice(&body).map_err(|_| {
            TokenError::Transport(format!("{endpoint} answered with something unexpected"))
        });
    }
    match serde_json::from_slice::<OauthErrorBody>(&body) {
        Ok(error) => Err(TokenError::Oauth {
            error: error.error,
            description: error.error_description,
        }),
        Err(_) => Err(TokenError::Transport(format!(
            "{endpoint} answered {status}"
        ))),
    }
}

pub(crate) async fn exchange_code(
    http: &reqwest::Client,
    metadata: &Metadata,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<Tokens, TokenError> {
    post_form(
        http,
        &metadata.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("resource", &metadata.issuer),
        ],
    )
    .await
}

pub(crate) async fn refresh(
    http: &reqwest::Client,
    metadata: &Metadata,
    refresh_token: &str,
) -> Result<Tokens, TokenError> {
    post_form(
        http,
        &metadata.token_endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT_ID),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

/// Revokes the whole Grant `token` belongs to.
pub(crate) async fn revoke(
    http: &reqwest::Client,
    metadata: &Metadata,
    token: &str,
) -> Result<(), TokenError> {
    let _: serde_json::Value = post_form(
        http,
        &metadata.revocation_endpoint,
        &[("token", token), ("client_id", CLIENT_ID)],
    )
    .await?;
    Ok(())
}

/// What the device shows the person and polls with.
#[derive(Deserialize)]
pub(crate) struct DeviceAuthorization {
    pub(crate) device_code: String,
    pub(crate) user_code: String,
    pub(crate) verification_uri: String,
    pub(crate) verification_uri_complete: String,
    pub(crate) expires_in: u64,
    pub(crate) interval: u64,
}

impl fmt::Debug for DeviceAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceAuthorization")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .finish_non_exhaustive()
    }
}

pub(crate) async fn start_device(
    http: &reqwest::Client,
    metadata: &Metadata,
) -> Result<DeviceAuthorization, TokenError> {
    post_form(
        http,
        &metadata.device_authorization_endpoint,
        &[
            ("client_id", CLIENT_ID),
            ("scope", "admin"),
            ("resource", &metadata.issuer),
        ],
    )
    .await
}

pub(crate) async fn poll_device(
    http: &reqwest::Client,
    metadata: &Metadata,
    device_code: &str,
) -> Result<Tokens, TokenError> {
    post_form(
        http,
        &metadata.token_endpoint,
        &[
            ("grant_type", DEVICE_GRANT),
            ("client_id", CLIENT_ID),
            ("device_code", device_code),
        ],
    )
    .await
}
