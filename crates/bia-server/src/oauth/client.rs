//! Who is asking: turns a `client_id` into a client with the redirect URIs it may use.
//!
//! A `client_id` that is an `https` URL names a Client ID Metadata Document, fetched from that
//! URL (see [`super::fetch`]). Any other is looked up among the clients that registered
//! themselves. This is the one place a client id becomes a client, and where the built-in
//! client of the CLI is known (ADR-0068).

use axum::http::StatusCode;
use bia_store::oauth::{OAuthRepository, PgOAuthRepository, Provenance};
use chrono::TimeDelta;
use serde::Deserialize;
use url::{Host, Url};

use super::fetch::FetchError;
use crate::{ApiError, ApiState, ErrorCode};

const MAX_NAME_LEN: usize = 128;

/// The `client_id` of the CLI. It is not a URL, so it cannot name a metadata document, and
/// registration never hands it out.
pub(crate) const BUILTIN_CLI_ID: &str = "bia-cli";

/// The CLI is first-party and known without fetching or registration, since a metadata
/// document needs HTTPS and an install may be plain HTTP. Its one redirect URI is on loopback,
/// where [`redirect_uri_matches`] lets the CLI pick any port, and nothing else can be added.
fn builtin_cli() -> ResolvedClient {
    ResolvedClient {
        client_id: BUILTIN_CLI_ID.to_owned(),
        name: "bia CLI (built in)".to_owned(),
        host: None,
        provenance: Provenance::Builtin,
        redirect_uris: vec!["http://127.0.0.1/callback".to_owned()],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedClient {
    pub(crate) client_id: String,
    pub(crate) name: String,
    /// The host a metadata document came from.
    pub(crate) host: Option<String>,
    pub(crate) provenance: Provenance,
    pub(crate) redirect_uris: Vec<String>,
}

/// The client `client_id` names, or the error to show the person: nothing is redirected to a
/// client that cannot be identified.
pub(crate) async fn resolve(state: &ApiState, client_id: &str) -> Result<ResolvedClient, ApiError> {
    if client_id == BUILTIN_CLI_ID {
        return Ok(builtin_cli());
    }
    if client_id.starts_with("https://") || client_id.starts_with("http://") {
        return from_metadata_document(state, client_id).await;
    }
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let client = PgOAuthRepository
        .client(&mut conn, client_id)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidClient,
                "no client has this `client_id`; register with `POST /oauth/register` or use a \
                 Client ID Metadata Document URL",
            )
        })?;
    Ok(ResolvedClient {
        client_id: client.client_id,
        name: client.client_name,
        host: None,
        provenance: Provenance::Dynamic,
        redirect_uris: client.redirect_uris,
    })
}

#[derive(Deserialize)]
struct MetadataDocument {
    client_id: String,
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: Option<String>,
}

async fn from_metadata_document(
    state: &ApiState,
    client_id: &str,
) -> Result<ResolvedClient, ApiError> {
    let now = state.clock.now();
    if let Some(client) = state.metadata.get(client_id, now) {
        return Ok(client);
    }
    let url = metadata_url(client_id)?;
    let fetched = state
        .config
        .client_metadata_fetcher
        .fetch(&url)
        .await
        .map_err(|failure| match failure {
            FetchError::Blocked => refuse(
                ErrorCode::ClientMetadataBlocked,
                "the client's metadata URL must be HTTPS and must not resolve to a loopback, \
                 private or link-local address",
            ),
            FetchError::Unreachable => refuse(
                ErrorCode::ClientMetadataUnreachable,
                "the client's metadata document could not be fetched in time with status 200",
            ),
            FetchError::TooLarge => refuse(
                ErrorCode::ClientMetadataTooLarge,
                "the client's metadata document is larger than 5 KB",
            ),
            FetchError::Redirected => refuse(
                ErrorCode::ClientMetadataRedirected,
                "the client's metadata URL redirects; bia does not follow redirects",
            ),
        })?;
    let invalid = |message: &str| refuse(ErrorCode::ClientMetadataInvalid, message);
    let document: MetadataDocument = serde_json::from_slice(&fetched.body)
        .map_err(|_| invalid("the client's metadata document is not the expected JSON object"))?;
    if document.client_id != client_id {
        return Err(refuse(
            ErrorCode::ClientMetadataMismatch,
            "the `client_id` in the metadata document is not the URL it was fetched from",
        ));
    }
    if document
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|method| method != "none")
    {
        return Err(invalid(
            "bia's clients are public: `token_endpoint_auth_method` must be `none`",
        ));
    }
    if document.redirect_uris.is_empty() {
        return Err(invalid("the metadata document lists no `redirect_uris`"));
    }
    if let Some(bad) = document
        .redirect_uris
        .iter()
        .find(|uri| validate_redirect_uri(uri).is_err())
    {
        return Err(invalid(&format!(
            "`{bad}` is not an acceptable redirect URI"
        )));
    }
    let host = url.host_str().unwrap_or_default().to_owned();
    let client = ResolvedClient {
        client_id: client_id.to_owned(),
        name: display_name(document.client_name.as_deref()).unwrap_or_else(|| host.clone()),
        host: Some(host),
        provenance: Provenance::Cimd,
        redirect_uris: document.redirect_uris,
    };
    if let Some(max_age) = fetched.max_age
        && let Ok(max_age) = TimeDelta::from_std(max_age)
    {
        state.metadata.put(&client, now + max_age, now);
    }
    Ok(client)
}

fn refuse(code: ErrorCode, message: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, code, message)
}

/// A client id URL has a path, no fragment or credentials, and no dot segments (draft
/// client ID metadata document, section 3).
fn metadata_url(client_id: &str) -> Result<Url, ApiError> {
    let invalid = || {
        refuse(
            ErrorCode::ClientMetadataInvalid,
            "a `client_id` that is a URL must have a path, and no fragment, credentials or dot \
             segments",
        )
    };
    let url = Url::parse(client_id).map_err(|_| invalid())?;
    let dotted = url
        .path_segments()
        .is_some_and(|mut segments| segments.any(|s| s == "." || s == ".."));
    if url.path() == "/"
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || dotted
        || url.host().is_none()
    {
        return Err(invalid());
    }
    Ok(url)
}

/// A name a person will read: printable, one line, bounded.
pub(crate) fn display_name(name: Option<&str>) -> Option<String> {
    let name: String = name?
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_LEN)
        .collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Schemes a redirect must never use: they run code or read files in the user's browser.
const FORBIDDEN_SCHEMES: &[&str] = &[
    "javascript",
    "data",
    "vbscript",
    "file",
    "blob",
    "about",
    "ftp",
    "ws",
    "wss",
];

/// Whether a client may register `uri` as a redirect URI: `https`, `http` on loopback, or the
/// private-use scheme of a native app; never a fragment or credentials.
pub(crate) fn validate_redirect_uri(uri: &str) -> Result<(), &'static str> {
    let url = Url::parse(uri).map_err(|_| "it is not an absolute URI")?;
    if url.fragment().is_some() {
        return Err("it has a fragment");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("it has credentials");
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(&url) => Ok(()),
        "http" => Err("`http` is only for loopback addresses"),
        scheme if FORBIDDEN_SCHEMES.contains(&scheme) => Err("its scheme is not allowed"),
        _ => Ok(()),
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(name)) => name == "localhost",
        None => false,
    }
}

/// Whether `requested` is one of the `registered` redirect URIs: compared exactly, except that
/// a registered `http` loopback URI matches any port (RFC 8252 section 7.3).
pub(crate) fn redirect_uri_matches(registered: &[String], requested: &str) -> bool {
    if validate_redirect_uri(requested).is_err() {
        return false;
    }
    if registered.iter().any(|uri| uri == requested) {
        return true;
    }
    let Ok(requested) = Url::parse(requested) else {
        return false;
    };
    requested.scheme() == "http"
        && is_loopback(&requested)
        && registered.iter().any(|uri| {
            Url::parse(uri).is_ok_and(|registered| {
                registered.scheme() == "http"
                    && is_loopback(&registered)
                    && registered.host() == requested.host()
                    && registered.path() == requested.path()
                    && registered.query() == requested.query()
            })
        })
}

#[cfg(test)]
mod tests {
    use super::{redirect_uri_matches, validate_redirect_uri};

    fn matches(registered: &[&str], requested: &str) -> bool {
        let registered: Vec<String> = registered.iter().map(|s| (*s).to_owned()).collect();
        redirect_uri_matches(&registered, requested)
    }

    #[test]
    fn redirect_uris_match_exactly() {
        let registered = ["https://app.example/callback"];

        assert!(matches(&registered, "https://app.example/callback"));
        assert!(!matches(&registered, "https://app.example/callback/"));
        assert!(!matches(&registered, "https://app.example/callback?x=1"));
        assert!(!matches(&registered, "https://app.example/other"));
        assert!(!matches(&registered, "https://app.example.evil/callback"));
        assert!(!matches(&registered, "http://app.example/callback"));
        assert!(!matches(&registered, "https://app.example:8443/callback"));
    }

    #[test]
    fn a_loopback_redirect_may_use_any_port_and_nothing_else_may_differ() {
        let registered = ["http://127.0.0.1/callback", "http://localhost:3000/cb"];

        assert!(matches(&registered, "http://127.0.0.1:51234/callback"));
        assert!(matches(&registered, "http://127.0.0.1/callback"));
        assert!(matches(&registered, "http://localhost:9/cb"));
        assert!(!matches(&registered, "http://127.0.0.1:51234/other"));
        assert!(!matches(&registered, "http://localhost:9/callback"));
        assert!(!matches(&registered, "http://[::1]:9/callback"));
        assert!(!matches(&registered, "https://127.0.0.1:51234/callback"));
        assert!(!matches(
            &registered,
            "http://127.0.0.1.evil.example/callback"
        ));
    }

    #[test]
    fn the_built_in_cli_client_accepts_loopback_redirects_only() {
        let cli = super::builtin_cli();

        assert!(redirect_uri_matches(
            &cli.redirect_uris,
            "http://127.0.0.1:41234/callback"
        ));
        for other in [
            "https://127.0.0.1:41234/callback",
            "http://127.0.0.1:41234/other",
            "http://localhost:41234/callback",
            "http://example.com/callback",
            "https://example.com/callback",
            "http://127.0.0.1.evil.example/callback",
            "cursor://anysphere.cursor/oauth",
        ] {
            assert!(!redirect_uri_matches(&cli.redirect_uris, other), "{other}");
        }
    }

    #[test]
    fn a_non_loopback_port_never_matches_by_loophole() {
        assert!(!matches(
            &["http://example.com/cb"],
            "http://example.com:8080/cb"
        ));
        assert!(!matches(
            &["https://app.example/cb"],
            "https://app.example:1/cb"
        ));
    }

    #[test]
    fn redirect_uris_that_run_code_or_leak_are_refused() {
        for uri in [
            "javascript:alert(1)",
            "data:text/html,x",
            "https://app.example/cb#frag",
            "https://user:pass@app.example/cb",
            "http://app.example/cb",
            "/relative",
        ] {
            assert!(validate_redirect_uri(uri).is_err(), "{uri}");
        }
        for uri in [
            "https://app.example/cb",
            "http://127.0.0.1:8080/cb",
            "http://[::1]/cb",
            "cursor://anysphere.cursor/oauth",
        ] {
            assert!(validate_redirect_uri(uri).is_ok(), "{uri}");
        }
    }
}
