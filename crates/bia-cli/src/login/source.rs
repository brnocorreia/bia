//! The Token an API command sends: `BIA_TOKEN` as given, or the stored login, whose access
//! token is renewed with its refresh token before it expires and after a 401.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bia_client::TokenSource;
use bia_core::clock::Clock;
use chrono::TimeDelta;

use super::credentials::{Credentials, StoredLogin};
use super::oauth::{self, TokenError};

/// How long before it expires an access token is replaced, so a request that takes a moment
/// does not arrive with a dead one.
const REFRESH_MARGIN: TimeDelta = TimeDelta::seconds(60);

/// `BIA_TOKEN` or the config file's `token`: it wins over a login and is never refreshed.
pub(crate) struct StaticToken(pub(crate) String);

impl TokenSource for StaticToken {
    fn token(&self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(std::future::ready(Some(self.0.clone())))
    }

    fn refresh(
        &self,
        _: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(std::future::ready(None))
    }
}

/// The login `bia login` stored for one server.
pub(crate) struct StoredLoginSource {
    credentials: Arc<Credentials>,
    url: String,
    http: reqwest::Client,
    clock: Arc<dyn Clock>,
}

impl StoredLoginSource {
    pub(crate) fn new(
        credentials: Arc<Credentials>,
        url: String,
        http: reqwest::Client,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            credentials,
            url,
            http,
            clock,
        }
    }

    fn is_fresh(&self, login: &StoredLogin) -> bool {
        login.expires_at - REFRESH_MARGIN > self.clock.now()
    }

    fn load(&self) -> Option<StoredLogin> {
        self.credentials.load(&self.url).ok().flatten()
    }

    /// A working access token, renewing it first when `rejected` (the one the server
    /// refused) is still the stored one, or when none was refused and it is about to expire.
    ///
    /// Under a lock, because a refresh token is single-use: a second process refreshing with
    /// a spent one would revoke the Grant. The process that waited finds the new login
    /// stored and uses it.
    async fn renew(&self, rejected: Option<&str>) -> Option<String> {
        let credentials = self.credentials.clone();
        let _lock = tokio::task::spawn_blocking(move || credentials.lock())
            .await
            .ok()?
            .ok()?;
        let login = self.load()?;
        let someone_else_renewed = rejected.is_none_or(|rejected| login.access_token != rejected);
        if someone_else_renewed && self.is_fresh(&login) {
            return Some(login.access_token);
        }
        let metadata = oauth::discover(&self.http, &self.url).await.ok()?;
        match oauth::refresh(&self.http, &metadata, &login.refresh_token).await {
            Ok(tokens) => {
                let renewed = StoredLogin {
                    access_token: tokens.access_token,
                    refresh_token: tokens.refresh_token,
                    expires_at: self.clock.now() + TimeDelta::seconds(tokens.expires_in),
                    scope: if tokens.scope.is_empty() {
                        login.scope
                    } else {
                        tokens.scope
                    },
                };
                self.credentials.save(&self.url, &renewed).ok()?;
                Some(renewed.access_token)
            }
            // The Grant was revoked or the refresh token ran out: nothing stored can work
            // again, so stop trying and let the 401 say to log in.
            Err(TokenError::Oauth { error, .. }) if error == "invalid_grant" => {
                let _ = self.credentials.delete(&self.url);
                None
            }
            Err(_) => None,
        }
    }
}

impl TokenSource for StoredLoginSource {
    fn token(&self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(async move {
            let login = self.load()?;
            if self.is_fresh(&login) {
                return Some(login.access_token);
            }
            self.renew(None).await
        })
    }

    fn refresh(
        &self,
        rejected: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(async move { self.renew(rejected.as_deref()).await })
    }
}
