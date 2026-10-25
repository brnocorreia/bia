//! Typed HTTP client for the bia API.
//!
//! Every method maps to one endpoint. A failed request comes back as [`Error::Api`] carrying
//! the server's error body, Diagnostics included. Every endpoint but the probes needs a Token,
//! set with [`Client::with_token`].

use std::fmt;

use bia_core::validation::Diagnostic;
use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("`{0}` is not a valid base URL")]
    BaseUrl(String),
    /// The server answered with its error body.
    #[error("{} ({}): {}", .body.code, .status, .body.message)]
    Api { status: StatusCode, body: ErrorBody },
    /// No usable answer: the server is unreachable, or answered with something that is not
    /// the API's.
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
}

/// The body of every API error response.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ErrorBody {
    /// A stable snake_case code, e.g. `route_not_found` or `invalid_definition`.
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Status {
    pub status: String,
}

/// `GET /v1/schema`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SchemaDocument {
    pub definition: Value,
    pub queue: Value,
    pub actions: Vec<ActionEntry>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ActionEntry {
    pub name: String,
    pub category: String,
    pub description: String,
    /// `pure` or `side_effecting`.
    pub effect: String,
    pub dangerous: bool,
    pub enabled: bool,
    pub param_schema: Value,
    pub output_schema: Value,
    pub examples: Vec<ActionExample>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ActionExample {
    pub description: String,
    pub params: Value,
    pub output: Value,
}

/// What a Token may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Admin,
    Agent,
    Webhook,
}

/// The webhook Trigger a `webhook` Token is bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookBinding {
    pub workflow_id: String,
    pub trigger_id: String,
}

/// `POST /v1/tokens`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewToken {
    pub name: String,
    pub scope: Scope,
    /// Required with [`Scope::Webhook`], refused otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookBinding>,
}

/// A Token, without its value.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Token {
    pub id: Uuid,
    pub name: String,
    pub scope: Scope,
    pub webhook: Option<WebhookBinding>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A Token just created, with the value the API shows only this once.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct CreatedToken {
    pub token: Token,
    pub value: String,
}

impl fmt::Debug for CreatedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedToken")
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct TokenList {
    tokens: Vec<Token>,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
    token: Option<String>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

impl Client {
    /// `base_url` is where the API is served, e.g. `http://localhost:8080`.
    pub fn new(base_url: &str) -> Result<Self, Error> {
        let mut base = Url::parse(base_url).map_err(|_| Error::BaseUrl(base_url.to_owned()))?;
        if base.cannot_be_a_base() {
            return Err(Error::BaseUrl(base_url.to_owned()));
        }
        // Without it, joining a route would replace the base path's last segment.
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            http: reqwest::Client::new(),
            base,
            token: None,
        })
    }

    /// Sends `token` as `Authorization: Bearer <token>` with every request.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub async fn health(&self) -> Result<Status, Error> {
        self.send(Method::GET, "/health", None::<&()>).await
    }

    pub async fn ready(&self) -> Result<Status, Error> {
        self.send(Method::GET, "/ready", None::<&()>).await
    }

    pub async fn schema(&self) -> Result<SchemaDocument, Error> {
        self.send(Method::GET, "/v1/schema", None::<&()>).await
    }

    pub async fn create_token(&self, token: &NewToken) -> Result<CreatedToken, Error> {
        self.send(Method::POST, "/v1/tokens", Some(token)).await
    }

    pub async fn list_tokens(&self) -> Result<Vec<Token>, Error> {
        let list: TokenList = self.send(Method::GET, "/v1/tokens", None::<&()>).await?;
        Ok(list.tokens)
    }

    pub async fn revoke_token(&self, id: Uuid) -> Result<Token, Error> {
        self.send(
            Method::POST,
            &format!("/v1/tokens/{id}/revoke"),
            None::<&()>,
        )
        .await
    }

    /// Mints the first admin Token of a fresh install with the server's bootstrap key, which
    /// is sent instead of this client's Token.
    pub async fn bootstrap_token(
        &self,
        bootstrap_key: &str,
        name: &str,
    ) -> Result<CreatedToken, Error> {
        #[derive(Serialize)]
        struct Request<'a> {
            name: &'a str,
        }
        let client = self.clone().with_token(bootstrap_key);
        client
            .send(
                Method::POST,
                "/v1/tokens/bootstrap",
                Some(&Request { name }),
            )
            .await
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T, Error> {
        let url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|_| Error::BaseUrl(self.base.to_string()))?;
        let mut request = self.http.request(method, url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response.json().await?);
        }
        Err(Error::Api {
            status,
            body: response.json().await?,
        })
    }
}
