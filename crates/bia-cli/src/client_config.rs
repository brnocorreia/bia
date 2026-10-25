//! Where the API commands find the server and their Token (ADR-0047).

use std::path::Path;

use anyhow::{Context, anyhow, bail};
use bia_client::Client;
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;

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

    pub(crate) fn client(&self, url_flag: Option<&str>) -> anyhow::Result<Client> {
        let url = self.url(url_flag);
        let client = Client::new(url).with_context(|| format!("invalid server URL `{url}`"))?;
        Ok(match self.token() {
            Some(token) => client.with_token(token),
            None => client,
        })
    }

    pub(crate) fn token(&self) -> Option<&str> {
        self.token.as_deref().filter(|token| !token.is_empty())
    }
}
