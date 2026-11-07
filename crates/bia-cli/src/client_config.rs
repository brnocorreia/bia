//! Where the API commands find the server and their Token (ADR-0047).

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, anyhow, bail};
use bia_client::{Client, TokenSource};
use bia_store::SystemClock;
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;

use crate::login::credentials::Credentials;
use crate::login::source::{StaticToken, StoredLoginSource};

pub(crate) const DEFAULT_URL: &str = "http://127.0.0.1:8080";

// No `Debug`: `token` is a credential.
#[derive(Default, Deserialize)]
pub(crate) struct ClientConfig {
    url: Option<String>,
    token: Option<String>,
}

impl ClientConfig {
    /// Reads `url` and `token` from `file` when given, then from `BIA_URL` and `BIA_TOKEN`.
    /// Other keys of the file belong to `bia serve`.
    pub(crate) fn load(file: Option<&Path>) -> anyhow::Result<Self> {
        let mut figment = Figment::new();
        if let Some(file) = file {
            if !file.is_file() {
                bail!("config file {} not found", file.display());
            }
            figment = figment.merge(Toml::file_exact(file));
        }
        figment
            .merge(Env::prefixed("BIA_").only(&["url", "token"]))
            .extract()
            .map_err(|error| anyhow!("invalid configuration: {error}"))
    }

    /// `url_flag`, else the configured URL, else [`DEFAULT_URL`].
    pub(crate) fn url<'a>(&'a self, url_flag: Option<&'a str>) -> &'a str {
        url_flag.or(self.url.as_deref()).unwrap_or(DEFAULT_URL)
    }

    /// A client that sends the configured Token, which wins, or else the login `bia login`
    /// stored for this server.
    pub(crate) fn client(&self, url_flag: Option<&str>) -> anyhow::Result<Client> {
        let url = self.url(url_flag);
        let client = Client::new(url).with_context(|| format!("invalid server URL `{url}`"))?;
        Ok(match (self.token(), self.stored_login(url)) {
            (Some(token), _) => client.with_token(token),
            (None, Some(source)) => client.with_token_source(source),
            (None, None) => client,
        })
    }

    /// Where `bia mcp` gets its Token: the configured one, else the stored login.
    pub(crate) fn token_source(&self, url_flag: Option<&str>) -> Option<Arc<dyn TokenSource>> {
        match self.token() {
            Some(token) => Some(Arc::new(StaticToken(token.to_owned()))),
            None => self
                .stored_login(self.url(url_flag))
                .map(|source| source as Arc<dyn TokenSource>),
        }
    }

    /// Whether a Token is configured or a login is stored for this server.
    pub(crate) fn has_credentials(&self, url_flag: Option<&str>) -> bool {
        self.token().is_some()
            || Credentials::from_env()
                .and_then(|credentials| credentials.load(self.url(url_flag)).ok().flatten())
                .is_some()
    }

    fn stored_login(&self, url: &str) -> Option<Arc<StoredLoginSource>> {
        Some(Arc::new(StoredLoginSource::new(
            Arc::new(Credentials::from_env()?),
            url.to_owned(),
            crate::login::http(),
            Arc::new(SystemClock),
        )))
    }

    pub(crate) fn token(&self) -> Option<&str> {
        self.token.as_deref().filter(|token| !token.is_empty())
    }
}
