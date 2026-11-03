//! The OAuth 2.1 authorization server that lets MCP clients sign themselves in (ADR-0067),
//! following the MCP authorization specification of 2026-07-28.
//!
//! A client finds the server through [`discovery`], names itself with a Client ID Metadata
//! Document ([`client`], [`fetch`]) or by registering ([`register`]), sends a person to
//! [`authorize`], who approves it on the UI's consent screen ([`consent`], which requires a
//! Session), and exchanges the code for tokens at [`token`]. The result is a Grant (ADR-0067)
//! whose access tokens are `oauth` Tokens limited to the Grant's `resource`.
//!
//! [`client::resolve`] is the one place a client id becomes a client, and
//! [`issue::issue_tokens`] turns any Grant into an access token and a refresh token. The CLI
//! uses both, through the authorization code flow or [`device`] (ADR-0068).

use std::collections::HashMap;
use std::sync::Mutex;

use axum::http::StatusCode;
use chrono::{DateTime, TimeDelta, Utc};

use crate::{ApiConfig, ApiError, ErrorCode};

pub(crate) mod authorize;
pub(crate) mod client;
pub(crate) mod consent;
pub(crate) mod device;
pub(crate) mod discovery;
mod error;
pub(crate) mod fetch;
pub(crate) mod issue;
pub(crate) mod pkce;
pub(crate) mod register;
pub(crate) mod revoke;
pub(crate) mod token;

pub use fetch::{ClientMetadataFetcher, Resolve};

pub(crate) use error::{OAuthError, OAuthErrorBody};

pub(crate) const ACCESS_TOKEN_LIFETIME: TimeDelta = TimeDelta::hours(1);
/// A refresh token unused for this long stops working. Each is used once and replaced, so this
/// is also how long one lives.
pub(crate) const REFRESH_TOKEN_LIFETIME: TimeDelta = TimeDelta::days(90);
/// How long a person has to answer the consent screen.
pub(crate) const AUTHORIZATION_LIFETIME: TimeDelta = TimeDelta::minutes(10);
/// How long a person has to enter the user code a device showed, and the device to collect the
/// result.
pub(crate) const DEVICE_LIFETIME: TimeDelta = TimeDelta::minutes(10);
/// How long a client has to exchange the code it was given.
pub(crate) const CODE_LIFETIME: TimeDelta = TimeDelta::seconds(60);

pub(crate) const AUTHORIZATION_CODE_PREFIX: &str = "bia_code_";
pub(crate) const REFRESH_TOKEN_PREFIX: &str = "bia_refresh_";
pub(crate) const DEVICE_CODE_PREFIX: &str = "bia_device_";

const DEFAULT_DEVICE_INTERVAL_SECS: i32 = 5;

impl ApiConfig {
    /// `public_url` without a trailing slash: the issuer, and the `resource` of tokens that work
    /// on the whole API.
    pub(crate) fn issuer(&self) -> Result<String, ApiError> {
        self.public_url
            .as_ref()
            .map(|url| url.as_str().trim_end_matches('/').to_owned())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    ErrorCode::PublicUrlMissing,
                    "OAuth needs `public_url` (BIA_PUBLIC_URL): the address clients reach this \
                     server at",
                )
            })
    }

    /// The `resource` of tokens that work on `/mcp` only.
    pub(crate) fn mcp_resource(&self) -> Result<String, ApiError> {
        Ok(format!("{}/mcp", self.issuer()?))
    }

    /// `resource` as bia names it, or `None` when it names neither `{public_url}` nor
    /// `{public_url}/mcp`. Scheme and host are case-insensitive and one trailing slash is
    /// ignored.
    pub(crate) fn canonical_resource(&self, requested: &str) -> Result<Option<String>, ApiError> {
        let Ok(url) = url::Url::parse(requested) else {
            return Ok(None);
        };
        if url.query().is_some() || url.fragment().is_some() {
            return Ok(None);
        }
        let canonical = url.as_str().trim_end_matches('/');
        Ok([self.issuer()?, self.mcp_resource()?]
            .into_iter()
            .find(|resource| resource == canonical))
    }

    /// Whether a Token issued for `audience` may be used on this request: an `/mcp` audience
    /// only through `/mcp`, the whole API's anywhere.
    pub(crate) fn audience_allows(&self, audience: &str, via_mcp: bool) -> bool {
        match (self.issuer(), self.mcp_resource()) {
            (Ok(issuer), Ok(mcp)) => audience == issuer || (via_mcp && audience == mcp),
            _ => false,
        }
    }

    /// Where a person types the user code of a device, or `None` when there is no UI.
    pub(crate) fn device_verification_url(&self) -> Option<String> {
        let ui = self.ui_url.as_ref()?;
        Some(format!("{}/device", ui.as_str().trim_end_matches('/')))
    }

    /// How long a device must leave between polls.
    pub(crate) fn device_interval_secs(&self) -> i32 {
        self.device_poll_interval
            .map_or(DEFAULT_DEVICE_INTERVAL_SECS, |interval| {
                i32::try_from(interval.as_secs().max(1)).unwrap_or(i32::MAX)
            })
    }

    /// Where the consent screen for an authorization is, or `None` when there is no UI.
    pub(crate) fn consent_url(&self, request: uuid::Uuid) -> Option<String> {
        let ui = self.ui_url.as_ref()?;
        Some(format!(
            "{}/oauth/consent?request={request}",
            ui.as_str().trim_end_matches('/')
        ))
    }
}

/// Clients fetched from their metadata URLs, kept while their cache headers allow.
#[derive(Debug, Default)]
pub(crate) struct MetadataCache(Mutex<HashMap<String, (client::ResolvedClient, DateTime<Utc>)>>);

const MAX_CACHED: usize = 256;

impl MetadataCache {
    pub(crate) fn get(
        &self,
        client_id: &str,
        now: DateTime<Utc>,
    ) -> Option<client::ResolvedClient> {
        let entries = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries
            .get(client_id)
            .filter(|(_, until)| now < *until)
            .map(|(client, _)| client.clone())
    }

    pub(crate) fn put(
        &self,
        client: &client::ResolvedClient,
        until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) {
        let mut entries = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, (_, until)| now < *until);
        if entries.len() >= MAX_CACHED {
            return;
        }
        entries.insert(client.client_id.clone(), (client.clone(), until));
    }
}
