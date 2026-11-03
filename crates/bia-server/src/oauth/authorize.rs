//! `GET /oauth/authorize`: validates the request and sends the person to the consent screen.
//!
//! Until the client and its redirect URI are known to be genuine, an error is shown to the
//! person instead of redirected. After that, every answer sent to the client, errors included,
//! carries `iss` (RFC 9207) so the client can tell which server answered.

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bia_store::oauth::{GrantScope, NewAuthorization, OAuthRepository, PgOAuthRepository};
use url::Url;

use super::client::{self, ResolvedClient};
use super::pkce;
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Query};

/// An authorization request. Parameters are listed here for the document; they are read from
/// the query string by hand so that a mistake can be answered the way OAuth requires.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[allow(dead_code)]
#[into_params(parameter_in = Query)]
pub(crate) struct AuthorizeParams {
    /// Must be `code`.
    response_type: Option<String>,
    /// A Client ID Metadata Document URL (`https`), or an id from `POST /oauth/register`.
    client_id: Option<String>,
    /// Exactly one of the client's registered redirect URIs; a loopback one may use any port.
    redirect_uri: Option<String>,
    /// The PKCE challenge: the base64url SHA-256 of the code verifier.
    code_challenge: Option<String>,
    /// Must be `S256`.
    code_challenge_method: Option<String>,
    /// The audience (RFC 8707): the server's `public_url`, or `{public_url}/mcp`.
    resource: Option<String>,
    /// `agent` (default) or `admin`; the person may approve less than is asked.
    scope: Option<String>,
    /// Returned unchanged.
    state: Option<String>,
}

/// Starts an authorization: redirects to the UI's consent screen at
/// `{ui_url}/oauth/consent?request=<id>`, where a signed-in person approves or denies. Needs no
/// credential. A problem with the client or its redirect URI is a 400 error body; any other
/// problem redirects to the client's `redirect_uri` with `error`, `error_description`, `state`
/// and `iss`.
#[utoipa::path(
    get,
    path = "/oauth/authorize",
    tag = "oauth",
    params(AuthorizeParams),
    responses(
        (status = FOUND, description = "To the consent screen, or to the client's `redirect_uri` with an OAuth `error`."),
        (status = BAD_REQUEST, description = "`invalid_client`, `invalid_redirect_uri`, `client_metadata_blocked`, `client_metadata_unreachable`, `client_metadata_too_large`, `client_metadata_redirected`, `client_metadata_invalid`, `client_metadata_mismatch`.", body = ErrorBody),
        (status = SERVICE_UNAVAILABLE, description = "`public_url_missing`, or, as plain text, a build without a UI and `ui_url`.", body = ErrorBody),
    )
)]
pub(crate) async fn authorize(
    State(state): State<ApiState>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    let issuer = state.config.issuer()?;
    let request = Request::new(&query);

    let Some(client_id) = request.single("client_id") else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::InvalidClient,
            "`client_id` is required, exactly once",
        ));
    };
    let client = client::resolve(&state, client_id).await?;
    let redirect_uri = match request.single("redirect_uri") {
        Some(uri) if client::redirect_uri_matches(&client.redirect_uris, uri) => uri.to_owned(),
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidRedirectUri,
                "`redirect_uri` must be exactly one of the client's registered redirect URIs",
            ));
        }
    };

    let answer = Answer {
        redirect_uri: &redirect_uri,
        state: request.single("state"),
        issuer: &issuer,
    };
    let checked = match check(&state, &request) {
        Ok(checked) => checked,
        Err(failure) => return Ok(answer.error(failure.error, failure.description)),
    };
    let Some(consent) = start(&state, &client, &redirect_uri, &request, checked).await? else {
        return Ok(unavailable_without_ui());
    };
    let mut response = StatusCode::FOUND.into_response();
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&consent).map_err(|e| ApiError::internal(&e))?,
    );
    Ok(response)
}

struct Checked {
    code_challenge: String,
    resource: String,
    scope: GrantScope,
}

struct Failure {
    error: &'static str,
    description: &'static str,
}

fn fail(error: &'static str, description: &'static str) -> Failure {
    Failure { error, description }
}

/// What the client asked for, or what to tell it is wrong.
fn check(state: &ApiState, request: &Request<'_>) -> Result<Checked, Failure> {
    if request.duplicated() {
        return Err(fail(
            "invalid_request",
            "a parameter was sent more than once",
        ));
    }
    if request.single("response_type") != Some("code") {
        return Err(fail(
            "unsupported_response_type",
            "`response_type` must be `code`",
        ));
    }
    let Some(code_challenge) = request.single("code_challenge") else {
        return Err(fail(
            "invalid_request",
            "`code_challenge` is required: PKCE is mandatory",
        ));
    };
    if request.single("code_challenge_method") != Some("S256") {
        return Err(fail(
            "invalid_request",
            "`code_challenge_method` must be `S256`; `plain` is not accepted",
        ));
    }
    if !pkce::is_challenge(code_challenge) {
        return Err(fail(
            "invalid_request",
            "`code_challenge` must be the base64url SHA-256 of the code verifier",
        ));
    }
    let Some(resource) = request.single("resource") else {
        return Err(fail(
            "invalid_target",
            "`resource` is required: the server's `public_url` or `{public_url}/mcp`",
        ));
    };
    let resource = match state.config.canonical_resource(resource) {
        Ok(Some(resource)) => resource,
        Ok(None) => {
            return Err(fail(
                "invalid_target",
                "`resource` is not this server: use its `public_url` or `{public_url}/mcp`",
            ));
        }
        Err(_) => return Err(fail("server_error", "the server has no `public_url`")),
    };
    let scope = match scope(request.single("scope")) {
        Ok(scope) => scope,
        Err(description) => return Err(fail("invalid_scope", description)),
    };
    Ok(Checked {
        code_challenge: code_challenge.to_owned(),
        resource,
        scope,
    })
}

/// `admin` if it is asked for, else `agent`, which is also what no `scope` means. Scopes bia
/// does not know are refused, but `offline_access` is ignored: refresh tokens are always issued.
fn scope(requested: Option<&str>) -> Result<GrantScope, &'static str> {
    let mut scope = GrantScope::Agent;
    for name in requested.unwrap_or_default().split_whitespace() {
        match name {
            "admin" => scope = GrantScope::Admin,
            "agent" | "offline_access" => {}
            _ => return Err("`scope` may name `agent` and `admin`"),
        }
    }
    Ok(scope)
}

/// Records the pending request and returns where the consent screen is, or `None` when the
/// server has no UI to send the person to.
async fn start(
    state: &ApiState,
    client: &ResolvedClient,
    redirect_uri: &str,
    request: &Request<'_>,
    checked: Checked,
) -> Result<Option<String>, ApiError> {
    if state.config.consent_url(uuid::Uuid::nil()).is_none() {
        return Ok(None);
    }
    let now = state.clock.now();
    let mut conn = state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let pending = PgOAuthRepository
        .begin_authorization(
            &mut conn,
            &NewAuthorization {
                client_id: client.client_id.clone(),
                client_name: client.name.clone(),
                client_host: client.host.clone(),
                provenance: client.provenance,
                redirect_uri: redirect_uri.to_owned(),
                requested_scope: checked.scope,
                resource: checked.resource,
                state: request.single("state").map(str::to_owned),
                code_challenge: checked.code_challenge,
                created_at: now,
                expires_at: now + super::AUTHORIZATION_LIFETIME,
            },
        )
        .await
        .map_err(|e| ApiError::internal(&e))?;
    Ok(state.config.consent_url(pending.id))
}

/// A page for a person whose server has nowhere to ask for consent.
fn unavailable_without_ui() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "bia cannot ask you to approve this app: it was built without the UI and `ui_url` \
         (BIA_UI_URL) is not set. Set `ui_url` to where the UI is served, or use a build with \
         the UI.\n",
    )
        .into_response()
}

/// The query string, with the duplicate check OAuth asks for.
struct Request<'a>(&'a [(String, String)]);

impl<'a> Request<'a> {
    fn new(query: &'a [(String, String)]) -> Self {
        Self(query)
    }

    /// The parameter's value if it was sent exactly once and is not empty.
    fn single(&self, name: &str) -> Option<&str> {
        let mut found = self.0.iter().filter(|(key, _)| key == name);
        let first = found.next()?;
        found
            .next()
            .is_none()
            .then_some(first.1.as_str())
            .filter(|value| !value.is_empty())
    }

    fn duplicated(&self) -> bool {
        self.0
            .iter()
            .enumerate()
            .any(|(i, (key, _))| self.0[..i].iter().any(|(other, _)| other == key))
    }
}

/// Where a response to the client goes: its redirect URI, with the issuer and its `state`.
pub(crate) struct Answer<'a> {
    pub(crate) redirect_uri: &'a str,
    pub(crate) state: Option<&'a str>,
    pub(crate) issuer: &'a str,
}

impl Answer<'_> {
    pub(crate) fn error(&self, error: &str, description: &str) -> Response {
        self.redirect(&[("error", error), ("error_description", description)])
    }

    /// The redirect URI, whose own query is kept, plus `parameters`, `state` and `iss`.
    pub(crate) fn url(&self, parameters: &[(&str, &str)]) -> Option<String> {
        let mut url = Url::parse(self.redirect_uri).ok()?;
        {
            let mut query = url.query_pairs_mut();
            for (name, value) in parameters {
                query.append_pair(name, value);
            }
            if let Some(state) = self.state {
                query.append_pair("state", state);
            }
            query.append_pair("iss", self.issuer);
        }
        Some(url.into())
    }

    pub(crate) fn redirect(&self, parameters: &[(&str, &str)]) -> Response {
        let Some(location) = self
            .url(parameters)
            .and_then(|url| HeaderValue::from_str(&url).ok())
        else {
            return ApiError::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidRedirectUri,
                "the redirect URI cannot carry a response",
            )
            .into_response();
        };
        let mut response = StatusCode::FOUND.into_response();
        response.headers_mut().insert(header::LOCATION, location);
        response
    }
}
