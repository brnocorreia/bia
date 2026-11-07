//! `bia login` and `bia logout` (ADR-0068): the CLI as an OAuth client of its own server.
//!
//! `login` runs the authorization code flow with PKCE and a loopback redirect, or with
//! `--device` the device flow for a machine without a browser, and stores the tokens
//! ([`credentials`]). Every later command and `bia mcp` send the stored access token through
//! [`source`], which renews it on its own.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bia_core::clock::Clock;
use chrono::TimeDelta;
use serde_json::json;
use sha2::{Digest, Sha256};

pub(crate) mod credentials;
mod loopback;
mod oauth;
pub(crate) mod source;

use credentials::{Credentials, Stored, StoredLogin};
use oauth::TokenError;

/// How long `bia login` waits for the person to approve in the browser.
const BROWSER_PATIENCE: Duration = Duration::from_secs(5 * 60);
/// What a device that polled too fast adds to its interval (RFC 8628 section 3.5).
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);

pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default()
}

pub(crate) struct LoginOptions {
    pub(crate) url: String,
    pub(crate) device: bool,
    pub(crate) no_browser: bool,
    pub(crate) json: bool,
    /// `BIA_TOKEN` is set, so it will be used instead of what this stores.
    pub(crate) token_overrides: bool,
}

pub(crate) async fn login(
    credentials: &Credentials,
    clock: &Arc<dyn Clock>,
    options: &LoginOptions,
) -> anyhow::Result<()> {
    let http = http();
    let metadata = oauth::discover(&http, &options.url).await?;
    let tokens = if options.device {
        device_flow(&http, &metadata).await?
    } else {
        code_flow(&http, &metadata, options.no_browser).await?
    };
    let previous = credentials.load(&options.url)?;
    let login = StoredLogin {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at: clock.now() + TimeDelta::seconds(tokens.expires_in),
        scope: tokens.scope,
    };
    let stored = credentials.save(&options.url, &login)?;
    if let Some(previous) = previous {
        // The Grant of the login this replaces would linger in Access, unused.
        let _ = oauth::revoke(&http, &metadata, &previous.refresh_token).await;
    }
    let (storage, place) = match &stored {
        Stored::Keychain => ("keychain", "the OS keychain".to_owned()),
        Stored::File(path) => ("file", path.display().to_string()),
    };
    if options.json {
        println!(
            "{}",
            json!({ "url": options.url, "scope": login.scope, "storage": storage })
        );
    } else {
        eprintln!(
            "Logged in to {} with scope `{}`; the login is kept in {place}.",
            options.url, login.scope
        );
    }
    if options.token_overrides {
        eprintln!(
            "note: BIA_TOKEN (or `token` in the config file) is set, and it wins over this login"
        );
    }
    Ok(())
}

async fn code_flow(
    http: &reqwest::Client,
    metadata: &oauth::Metadata,
    no_browser: bool,
) -> anyhow::Result<oauth::Tokens> {
    let (listener, redirect_uri) = loopback::bind()
        .await
        .context("cannot listen on a loopback port for the browser's redirect")?;
    let verifier = random_string(32)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_string(16)?;
    let mut authorize = url::Url::parse(&metadata.authorization_endpoint)
        .context("the server's authorization endpoint is not a URL")?;
    authorize
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", oauth::CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", &metadata.issuer)
        .append_pair("scope", "admin")
        .append_pair("state", &state);
    eprintln!(
        "Approve the sign-in in your browser. If it does not open, visit:\n\n  {authorize}\n"
    );
    if !no_browser && webbrowser::open(authorize.as_str()).is_err() {
        eprintln!("(bia could not open a browser here)");
    }
    let code = tokio::time::timeout(
        BROWSER_PATIENCE,
        loopback::wait_for_code(&listener, &state, &metadata.issuer),
    )
    .await
    .map_err(|_| {
        anyhow!("nobody approved the sign-in within 5 minutes; run `bia login` again")
    })??;
    oauth::exchange_code(http, metadata, &code, &redirect_uri, &verifier)
        .await
        .map_err(|e| anyhow!("the server refused the code: {e}"))
}

async fn device_flow(
    http: &reqwest::Client,
    metadata: &oauth::Metadata,
) -> anyhow::Result<oauth::Tokens> {
    let device = oauth::start_device(http, metadata)
        .await
        .map_err(|e| anyhow!("cannot start the device sign-in: {e}"))?;
    eprintln!(
        "To sign in, open {} on any device where you are signed in to bia and enter the code:\n\n  {}\n\nOr open {} directly.",
        device.verification_uri, device.user_code, device.verification_uri_complete
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = Duration::from_secs(device.interval.max(1));
    loop {
        tokio::time::sleep(interval).await;
        match oauth::poll_device(http, metadata, &device.device_code).await {
            Ok(tokens) => return Ok(tokens),
            Err(TokenError::Oauth { error, .. }) => match error.as_str() {
                "authorization_pending" => {}
                "slow_down" => interval += SLOW_DOWN_STEP,
                "expired_token" => bail!("the code expired; run `bia login --device` again"),
                "access_denied" => bail!("sign-in was denied"),
                other => bail!("the server refused the device sign-in ({other})"),
            },
            Err(error @ TokenError::Transport(_)) => bail!("{error}"),
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("the code expired; run `bia login --device` again");
        }
    }
}

/// `bytes` random bytes as unpadded base64url: a PKCE verifier (32 gives 43 characters) or a
/// `state`.
fn random_string(bytes: usize) -> anyhow::Result<String> {
    let mut random = vec![0u8; bytes];
    getrandom::fill(&mut random).map_err(|e| anyhow!("cannot read random bytes: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}

pub(crate) async fn logout(credentials: &Credentials, url: &str, json: bool) -> anyhow::Result<()> {
    let Some(login) = credentials.load(url)? else {
        if json {
            println!("{}", json!({ "url": url, "logged_in": false }));
        } else {
            eprintln!("Not logged in to {url}.");
        }
        return Ok(());
    };
    // Revoke first, but delete the stored login whatever happens: a login that cannot be
    // revoked from here is still not one this machine should keep using.
    let http = http();
    let revoked = match oauth::discover(&http, url).await {
        Ok(metadata) => oauth::revoke(&http, &metadata, &login.refresh_token)
            .await
            .map_err(anyhow::Error::from),
        Err(error) => Err(error),
    };
    credentials.delete(url)?;
    match revoked {
        Ok(()) => {
            if json {
                println!(
                    "{}",
                    json!({ "url": url, "logged_in": false, "revoked": true })
                );
            } else {
                eprintln!("Logged out of {url}; its access was revoked.");
            }
            Ok(())
        }
        Err(error) => Err(anyhow!(
            "the stored login was removed, but its access could not be revoked on the server \
             ({error:#}); revoke it in Access in the UI"
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::random_string;

    #[test]
    fn a_pkce_verifier_is_43_url_safe_characters() {
        let verifier = random_string(32).unwrap();

        assert_eq!(verifier.len(), 43);
        assert!(
            verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(verifier, random_string(32).unwrap());
    }
}
