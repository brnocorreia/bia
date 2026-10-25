//! Typed HTTP client for the bia API.
//!
//! Every method maps to one endpoint. A failed request comes back as [`Error::Api`] carrying
//! the server's error body, Diagnostics included.

use bia_core::validation::Diagnostic;
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

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

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
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
        })
    }

    pub async fn health(&self) -> Result<Status, Error> {
        self.send(Method::GET, "/health").await
    }

    pub async fn ready(&self) -> Result<Status, Error> {
        self.send(Method::GET, "/ready").await
    }

    pub async fn schema(&self) -> Result<SchemaDocument, Error> {
        self.send(Method::GET, "/v1/schema").await
    }

    async fn send<T: DeserializeOwned>(&self, method: Method, path: &str) -> Result<T, Error> {
        let url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|_| Error::BaseUrl(self.base.to_string()))?;
        let response = self.http.request(method, url).send().await?;
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
