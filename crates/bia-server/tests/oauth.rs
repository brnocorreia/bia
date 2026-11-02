//! The OAuth authorization server (ADR-0067), driven as an MCP client would: discovery, a
//! Client ID Metadata Document served by `wiremock`, authorization with consent given by a
//! Session, tokens, an `/mcp` call, refresh and refresh reuse.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bia_client::Scope;
use bia_server::{ApiConfig, ClientMetadataFetcher};
use chrono::TimeDelta;
use reqwest::StatusCode;
use rmcp::ServiceExt;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

use common::{PUBLIC_URL, TestServer};

const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CALLBACK: &str = "http://127.0.0.1/callback";
const MCP_RESOURCE: &str = "http://bia.test/mcp";

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn config(ui: bool) -> ApiConfig {
    ApiConfig {
        public_url: Some(PUBLIC_URL.parse().unwrap()),
        ui_url: ui.then(|| PUBLIC_URL.parse().unwrap()),
        client_metadata_fetcher: ClientMetadataFetcher::allowing_local_http_for_tests(),
        ..ApiConfig::default()
    }
}

struct Env {
    server: TestServer,
    /// Serves the client's metadata document.
    cimd: MockServer,
    http: reqwest::Client,
}

async fn env(pool: PgPool) -> Env {
    env_with(pool, config(true)).await
}

async fn env_with(pool: PgPool, config: ApiConfig) -> Env {
    Env {
        server: TestServer::start_with(pool, config).await,
        cimd: MockServer::start().await,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
    }
}

/// What an authorization request carries; override a field to break it.
#[derive(Clone)]
struct Authorize {
    params: Vec<(String, String)>,
}

impl Authorize {
    fn new(client_id: &str, resource: &str) -> Self {
        Self {
            params: [
                ("response_type", "code"),
                ("client_id", client_id),
                ("redirect_uri", CALLBACK),
                ("code_challenge", &challenge(VERIFIER)),
                ("code_challenge_method", "S256"),
                ("resource", resource),
                ("state", "xyz"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        }
    }

    fn with(mut self, name: &str, value: &str) -> Self {
        self.params.retain(|(key, _)| key != name);
        self.params.push((name.to_owned(), value.to_owned()));
        self
    }

    fn without(mut self, name: &str) -> Self {
        self.params.retain(|(key, _)| key != name);
        self
    }
}

fn query(location: &str) -> HashMap<String, String> {
    Url::parse(location)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

impl Env {
    async fn publish_client_document(&self, document: &Value) -> String {
        Mock::given(method("GET"))
            .and(path("/client.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(document))
            .mount(&self.cimd)
            .await;
        format!("{}/client.json", self.cimd.uri())
    }

    async fn publish_default_client(&self) -> String {
        let id = format!("{}/client.json", self.cimd.uri());
        self.publish_client_document(&json!({
            "client_id": id,
            "client_name": "Test Client",
            "redirect_uris": [CALLBACK],
            "token_endpoint_auth_method": "none",
        }))
        .await
    }

    async fn authorize(&self, request: &Authorize) -> reqwest::Response {
        self.http
            .get(self.server.url("/oauth/authorize"))
            .query(&request.params)
            .send()
            .await
            .unwrap()
    }

    /// An authorization that was accepted: the id of the pending request.
    async fn pending(&self, request: &Authorize) -> String {
        let response = self.authorize(request).await;
        assert_eq!(
            response.status(),
            StatusCode::FOUND,
            "{:?}",
            response.text().await
        );
        let location = location(&response);
        assert!(
            location.starts_with("http://bia.test/oauth/consent?request="),
            "{location}"
        );
        query(&location)["request"].clone()
    }

    /// The Session, as the UI sends it.
    fn as_person(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("cookie", format!("bia_session={}", self.server.session))
            .header("origin", &self.server.origin)
    }

    async fn consent(&self, id: &str) -> reqwest::Response {
        self.as_person(
            self.http
                .get(self.server.url(&format!("/oauth/consent/{id}"))),
        )
        .send()
        .await
        .unwrap()
    }

    async fn approve(&self, id: &str, body: &Value) -> reqwest::Response {
        self.as_person(
            self.http
                .post(self.server.url(&format!("/oauth/consent/{id}/approve")))
                .json(body),
        )
        .send()
        .await
        .unwrap()
    }

    /// Approves as the request asked and returns the code the redirect carries.
    async fn approved_code(&self, id: &str) -> String {
        let response = self.approve(id, &json!({})).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        let redirect = query(body["redirect_to"].as_str().unwrap());
        assert_eq!(redirect["iss"], PUBLIC_URL);
        assert_eq!(redirect["state"], "xyz");
        redirect["code"].clone()
    }

    async fn token_request(&self, form: &[(&str, &str)]) -> reqwest::Response {
        self.http
            .post(self.server.url("/oauth/token"))
            .form(form)
            .send()
            .await
            .unwrap()
    }

    async fn exchange(&self, client_id: &str, code: &str) -> reqwest::Response {
        self.token_request(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("redirect_uri", CALLBACK),
            ("code_verifier", VERIFIER),
        ])
        .await
    }

    async fn refresh(&self, client_id: &str, refresh_value: &str) -> reqwest::Response {
        self.token_request(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_value),
        ])
        .await
    }

    /// The whole dance for `client_id`: tokens issued after the person approves as asked.
    async fn sign_in(&self, client_id: &str, resource: &str, scope: &str) -> Value {
        let id = self
            .pending(&Authorize::new(client_id, resource).with("scope", scope))
            .await;
        let code = self.approved_code(&id).await;
        let response = self.exchange(client_id, &code).await;
        assert_eq!(response.status(), StatusCode::OK);
        response.json().await.unwrap()
    }

    async fn get_status(&self, path: &str, bearer: &str) -> (StatusCode, Value) {
        let response = self
            .http
            .get(self.server.url(path))
            .bearer_auth(bearer)
            .send()
            .await
            .unwrap();
        (
            response.status(),
            response.json().await.unwrap_or(Value::Null),
        )
    }

    /// The status and code of the error an authorization request is refused with.
    async fn refused(&self, request: &Authorize) -> (StatusCode, String) {
        let response = self.authorize(request).await;
        self.error_code(response).await
    }

    async fn error_code(&self, response: reqwest::Response) -> (StatusCode, String) {
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        let code = body["code"]
            .as_str()
            .or(body["error"].as_str())
            .unwrap_or_default()
            .to_owned();
        (status, code)
    }
}

async fn mcp_tool_names(env: &Env, access: &str) -> Vec<String> {
    let config = StreamableHttpClientTransportConfig::with_uri(env.server.url("/mcp"))
        .auth_header(access.to_owned());
    let client = ().serve(StreamableHttpClientTransport::from_config(config)).await.unwrap();
    let tools = client.list_tools(None).await.unwrap();
    tools.tools.iter().map(|t| t.name.to_string()).collect()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_mcp_client_goes_from_discovery_to_refresh_and_a_reused_refresh_token_revokes_the_grant(
    pool: PgPool,
) {
    let env = env(pool).await;
    let http = &env.http;

    // The first request fails and says where to look.
    let unauthenticated = http.post(env.server.url("/mcp")).send().await.unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        unauthenticated.headers()["www-authenticate"],
        "Bearer resource_metadata=\"http://bia.test/.well-known/oauth-protected-resource/mcp\", scope=\"agent\""
    );
    let resource: Value = http
        .get(env.server.url("/.well-known/oauth-protected-resource/mcp"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resource["resource"], MCP_RESOURCE);
    assert_eq!(resource["authorization_servers"], json!([PUBLIC_URL]));
    let server: Value = http
        .get(env.server.url("/.well-known/oauth-authorization-server"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(server["issuer"], PUBLIC_URL);
    assert_eq!(
        server["authorization_response_iss_parameter_supported"],
        true
    );
    assert_eq!(server["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(server["scopes_supported"], json!(["agent", "admin"]));
    assert_eq!(server["client_id_metadata_document_supported"], true);
    assert_eq!(
        server["authorization_endpoint"],
        "http://bia.test/oauth/authorize"
    );

    // The client names itself with a metadata document and sends the person to consent.
    let client_id = env.publish_default_client().await;
    let pending = env
        .pending(&Authorize::new(&client_id, MCP_RESOURCE).with("scope", "agent"))
        .await;
    let consent: Value = env.consent(&pending).await.json().await.unwrap();
    assert_eq!(consent["client"]["name"], "Test Client");
    assert_eq!(consent["client"]["provenance"], "cimd");
    assert_eq!(consent["client"]["verified"], true);
    assert_eq!(consent["client"]["host"], "127.0.0.1");
    assert_eq!(consent["redirect_uri"], CALLBACK);
    assert_eq!(consent["requested_scope"], "agent");
    assert_eq!(consent["resource"], MCP_RESOURCE);

    let code = env.approved_code(&pending).await;
    let issued: Value = env.exchange(&client_id, &code).await.json().await.unwrap();
    assert_eq!(issued["token_type"], "Bearer");
    assert_eq!(issued["expires_in"], 3600);
    assert_eq!(issued["scope"], "agent");
    let access = issued["access_token"].as_str().unwrap().to_owned();
    let refresh_one = issued["refresh_token"].as_str().unwrap().to_owned();

    assert!(
        mcp_tool_names(&env, &access)
            .await
            .contains(&"list_workflows".to_owned())
    );

    // Refreshing rotates the refresh token.
    let second: Value = env
        .refresh(&client_id, &refresh_one)
        .await
        .json()
        .await
        .unwrap();
    let refresh_two = second["refresh_token"].as_str().unwrap().to_owned();
    let access_two = second["access_token"].as_str().unwrap().to_owned();
    assert_ne!(refresh_one, refresh_two);
    assert!(
        mcp_tool_names(&env, &access_two)
            .await
            .contains(&"list_workflows".to_owned())
    );

    // The old one coming back means it leaked: the Grant dies, with the newest tokens.
    let reused = env.refresh(&client_id, &refresh_one).await;
    assert_eq!(
        env.error_code(reused).await,
        (StatusCode::BAD_REQUEST, "invalid_grant".into())
    );
    let after = env.refresh(&client_id, &refresh_two).await;
    assert_eq!(
        env.error_code(after).await,
        (StatusCode::BAD_REQUEST, "invalid_grant".into())
    );
    let mcp = http
        .post(env.server.url("/mcp"))
        .bearer_auth(&access_two)
        .send()
        .await
        .unwrap();
    assert_eq!(mcp.status(), StatusCode::UNAUTHORIZED);
    assert!(
        mcp.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .starts_with("Bearer error=\"invalid_token\"")
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_client_that_registered_itself_is_marked_unverified_and_gets_the_same_flow(pool: PgPool) {
    let env = env(pool).await;
    let registered = env
        .http
        .post(env.server.url("/oauth/register"))
        .json(&json!({ "client_name": "Self Registered", "redirect_uris": ["http://127.0.0.1/callback"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), StatusCode::CREATED);
    let registered: Value = registered.json().await.unwrap();
    let client_id = registered["client_id"].as_str().unwrap().to_owned();
    assert_eq!(registered["token_endpoint_auth_method"], "none");

    // A loopback redirect may use any port.
    let on_a_port = Authorize::new(&client_id, PUBLIC_URL)
        .with("redirect_uri", "http://127.0.0.1:49152/callback");
    let pending = env.pending(&on_a_port).await;
    let consent: Value = env.consent(&pending).await.json().await.unwrap();
    assert_eq!(consent["client"]["provenance"], "dynamic");
    assert_eq!(consent["client"]["verified"], false);
    assert_eq!(consent["client"]["name"], "Self Registered");
    let body: Value = env
        .approve(&pending, &json!({}))
        .await
        .json()
        .await
        .unwrap();
    let code = query(body["redirect_to"].as_str().unwrap())["code"].clone();
    let exchange = env
        .token_request(&[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", &code),
            ("redirect_uri", "http://127.0.0.1:49152/callback"),
            ("code_verifier", VERIFIER),
        ])
        .await;
    assert_eq!(exchange.status(), StatusCode::OK);
    let issued: Value = exchange.json().await.unwrap();
    // A resource of `{public_url}` works on the API and on `/mcp`.
    let access = issued["access_token"].as_str().unwrap();
    assert_eq!(
        env.get_status("/v1/tokens/me", access).await.0,
        StatusCode::OK
    );
    assert!(
        mcp_tool_names(&env, access)
            .await
            .contains(&"list_workflows".to_owned())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn registration_refuses_unsafe_redirects_and_is_rate_limited(pool: PgPool) {
    let env = env(pool).await;
    let register = |body: Value| {
        env.http
            .post(env.server.url("/oauth/register"))
            .json(&body)
            .send()
    };

    for uri in [
        "javascript:alert(1)",
        "http://example.com/cb",
        "https://app.example/cb#x",
    ] {
        let response = register(json!({ "redirect_uris": [uri] })).await.unwrap();
        assert_eq!(
            env.error_code(response).await,
            (StatusCode::BAD_REQUEST, "invalid_redirect_uri".into()),
            "{uri}"
        );
    }
    let response = register(json!({ "redirect_uris": [] })).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let secret_auth = register(
        json!({ "redirect_uris": [CALLBACK], "token_endpoint_auth_method": "client_secret_basic" }),
    )
    .await
    .unwrap();
    assert_eq!(
        env.error_code(secret_auth).await.1,
        "invalid_client_metadata"
    );

    let mut limited = None;
    for _ in 0..25 {
        let response = register(json!({ "redirect_uris": [CALLBACK] }))
            .await
            .unwrap();
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            limited = Some(response);
            break;
        }
    }
    assert!(limited.unwrap().headers().contains_key("retry-after"));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_metadata_document_that_cannot_be_trusted_is_refused_with_its_own_code(pool: PgPool) {
    let env = env(pool).await;
    let id = format!("{}/client.json", env.cimd.uri());
    let request = Authorize::new(&id, MCP_RESOURCE);

    // The `client_id` inside must be the URL it was fetched from.
    env.publish_client_document(
        &json!({ "client_id": "https://other.example/c.json", "redirect_uris": [CALLBACK] }),
    )
    .await;
    assert_eq!(
        env.refused(&request).await,
        (StatusCode::BAD_REQUEST, "client_metadata_mismatch".into())
    );

    env.cimd.reset().await;
    env.publish_client_document(
        &json!({ "client_id": id, "redirect_uris": [CALLBACK], "padding": "x".repeat(6000) }),
    )
    .await;
    assert_eq!(
        env.refused(&request).await,
        (StatusCode::BAD_REQUEST, "client_metadata_too_large".into())
    );

    env.cimd.reset().await;
    Mock::given(path("/client.json"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "http://127.0.0.1/elsewhere"),
        )
        .mount(&env.cimd)
        .await;
    assert_eq!(
        env.refused(&request).await,
        (StatusCode::BAD_REQUEST, "client_metadata_redirected".into())
    );

    env.cimd.reset().await;
    env.publish_client_document(&json!({ "client_id": id }))
        .await;
    assert_eq!(
        env.refused(&request).await,
        (StatusCode::BAD_REQUEST, "client_metadata_invalid".into())
    );

    env.cimd.reset().await;
    Mock::given(path("/client.json"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&env.cimd)
        .await;
    assert_eq!(
        env.refused(&request).await,
        (
            StatusCode::BAD_REQUEST,
            "client_metadata_unreachable".into()
        )
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_production_fetcher_refuses_a_document_on_a_private_address(pool: PgPool) {
    // The default configuration is the strict fetcher: no relaxed one is reachable from it.
    let config = ApiConfig {
        client_metadata_fetcher: ClientMetadataFetcher::strict(),
        ..config(true)
    };
    let env = env_with(pool, config).await;

    for client_id in [
        "https://127.0.0.1/client.json",
        "https://[::1]/client.json",
        "https://169.254.169.254/client.json",
        "https://[::ffff:10.0.0.1]/client.json",
        // The test document server is plain HTTP on loopback.
        &format!("{}/client.json", env.cimd.uri()),
    ] {
        let response = env
            .authorize(&Authorize::new(client_id, MCP_RESOURCE))
            .await;
        assert_eq!(
            env.error_code(response).await,
            (StatusCode::BAD_REQUEST, "client_metadata_blocked".into()),
            "{client_id}"
        );
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_unacceptable_authorization_request_is_refused_and_the_client_hears_why_with_iss(
    pool: PgPool,
) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;
    let base = Authorize::new(&client_id, MCP_RESOURCE);

    let cases = [
        (
            "missing PKCE",
            base.clone().without("code_challenge"),
            "invalid_request",
        ),
        (
            "plain PKCE",
            base.clone().with("code_challenge_method", "plain"),
            "invalid_request",
        ),
        (
            "no PKCE method",
            base.clone().without("code_challenge_method"),
            "invalid_request",
        ),
        (
            "missing resource",
            base.clone().without("resource"),
            "invalid_target",
        ),
        (
            "foreign resource",
            base.clone().with("resource", "https://other.example/mcp"),
            "invalid_target",
        ),
        (
            "a path under the MCP resource",
            base.clone().with("resource", "http://bia.test/mcp/extra"),
            "invalid_target",
        ),
        (
            "unknown scope",
            base.clone().with("scope", "root"),
            "invalid_scope",
        ),
        (
            "implicit flow",
            base.clone().with("response_type", "token"),
            "unsupported_response_type",
        ),
    ];
    for (name, request, error) in cases {
        let response = env.authorize(&request).await;
        assert_eq!(response.status(), StatusCode::FOUND, "{name}");
        let answer = query(&location(&response));
        assert_eq!(answer["error"], error, "{name}");
        assert_eq!(answer["iss"], PUBLIC_URL, "{name}");
        assert_eq!(answer["state"], "xyz", "{name}");
        assert!(!answer.contains_key("code"), "{name}");
    }

    // Not a challenge of the right shape, and a repeated parameter.
    let short = env
        .authorize(&base.clone().with("code_challenge", "short"))
        .await;
    assert_eq!(query(&location(&short))["error"], "invalid_request");
    let mut repeated = base.clone();
    repeated.params.push(("state".into(), "again".into()));
    assert_eq!(
        query(&location(&env.authorize(&repeated).await))["error"],
        "invalid_request"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_redirect_uri_that_is_not_registered_is_never_redirected_to(pool: PgPool) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;
    let base = Authorize::new(&client_id, MCP_RESOURCE);

    for uri in [
        "https://evil.example/callback",
        "http://127.0.0.1:1234/other",
        "http://127.0.0.1.evil.example/callback",
    ] {
        let response = env.authorize(&base.clone().with("redirect_uri", uri)).await;
        assert_eq!(
            env.error_code(response).await,
            (StatusCode::BAD_REQUEST, "invalid_redirect_uri".into()),
            "{uri}"
        );
    }
    let missing = env.authorize(&base.clone().without("redirect_uri")).await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    let unknown = env
        .authorize(&Authorize::new("no-such-client", MCP_RESOURCE))
        .await;
    assert_eq!(env.error_code(unknown).await.1, "invalid_client");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_mcp_audience_token_is_refused_on_the_api_and_a_whole_api_one_works_on_both(
    pool: PgPool,
) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;

    let mcp_only = env.sign_in(&client_id, MCP_RESOURCE, "agent").await;
    let mcp_only = mcp_only["access_token"].as_str().unwrap();
    let (status, body) = env.get_status("/v1/workflows", mcp_only).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("wrong_audience"))
    );
    assert!(
        mcp_tool_names(&env, mcp_only)
            .await
            .contains(&"list_workflows".to_owned())
    );

    let whole = env.sign_in(&client_id, PUBLIC_URL, "agent").await;
    let whole = whole["access_token"].as_str().unwrap();
    assert_eq!(
        env.get_status("/v1/workflows", whole).await.0,
        StatusCode::OK
    );
    assert!(
        mcp_tool_names(&env, whole)
            .await
            .contains(&"list_workflows".to_owned())
    );

    // A Token with no audience, like every manual one, works everywhere it always did.
    assert_eq!(
        env.get_status("/v1/workflows", &env.server.admin_token)
            .await
            .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_consent_downgraded_to_agent_cannot_publish_and_cannot_be_raised_above_the_request(
    pool: PgPool,
) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;

    let pending = env
        .pending(&Authorize::new(&client_id, PUBLIC_URL).with("scope", "admin"))
        .await;
    let consent: Value = env.consent(&pending).await.json().await.unwrap();
    assert_eq!(consent["requested_scope"], "admin");
    let body: Value = env
        .approve(&pending, &json!({ "scope": "agent" }))
        .await
        .json()
        .await
        .unwrap();
    let code = query(body["redirect_to"].as_str().unwrap())["code"].clone();
    let issued: Value = env.exchange(&client_id, &code).await.json().await.unwrap();
    assert_eq!(issued["scope"], "agent");
    let access = issued["access_token"].as_str().unwrap();
    let publish = env
        .http
        .post(env.server.url("/v1/workflows/w/versions/1/publish"))
        .bearer_auth(access)
        .send()
        .await
        .unwrap();
    assert_eq!(
        env.error_code(publish).await,
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );

    // Approving more than was asked for fails, and the request stays pending.
    let modest = env.pending(&Authorize::new(&client_id, PUBLIC_URL)).await;
    let raised = env.approve(&modest, &json!({ "scope": "admin" })).await;
    assert_eq!(
        env.error_code(raised).await,
        (StatusCode::BAD_REQUEST, "scope_exceeds_request".into())
    );
    assert_eq!(env.consent(&modest).await.status(), StatusCode::OK);
    // The screen cannot change the client or the redirect either: unknown fields are refused.
    let tampered = env
        .approve(
            &modest,
            &json!({ "redirect_uri": "https://evil.example/", "client_id": "x" }),
        )
        .await;
    assert!(tampered.status().is_client_error());
    assert_eq!(env.consent(&modest).await.status(), StatusCode::OK);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn only_a_session_with_an_accepted_origin_can_answer_a_consent_and_only_once(pool: PgPool) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;
    let pending = env.pending(&Authorize::new(&client_id, MCP_RESOURCE)).await;
    let url = env.server.url(&format!("/oauth/consent/{pending}/approve"));

    let nobody = env.http.post(&url).json(&json!({})).send().await.unwrap();
    assert_eq!(nobody.status(), StatusCode::UNAUTHORIZED);
    let no_origin = env
        .http
        .post(&url)
        .header("cookie", format!("bia_session={}", env.server.session))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        env.error_code(no_origin).await,
        (StatusCode::FORBIDDEN, "origin_mismatch".into())
    );
    let foreign = env
        .http
        .post(&url)
        .header("cookie", format!("bia_session={}", env.server.session))
        .header("origin", "https://evil.example")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    // An admin Token that is not a Session cannot approve, nor can an agent.
    let admin = env.server.admin_token.clone();
    let by_token = env
        .http
        .post(&url)
        .bearer_auth(&admin)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        env.error_code(by_token).await,
        (StatusCode::FORBIDDEN, "not_a_session".into())
    );
    let agent = env.server.token(Scope::Agent).await;
    let by_agent = env
        .http
        .post(&url)
        .bearer_auth(&agent)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(by_agent.status(), StatusCode::FORBIDDEN);

    assert_eq!(
        env.approve(&pending, &json!({})).await.status(),
        StatusCode::OK
    );
    let again = env.approve(&pending, &json!({})).await;
    assert_eq!(
        env.error_code(again).await,
        (StatusCode::NOT_FOUND, "authorization_not_found".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn denying_sends_the_client_access_denied_with_state_and_iss(pool: PgPool) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;
    let pending = env.pending(&Authorize::new(&client_id, MCP_RESOURCE)).await;

    let response = env
        .as_person(
            env.http
                .post(env.server.url(&format!("/oauth/consent/{pending}/deny"))),
        )
        .send()
        .await
        .unwrap();

    let body: Value = response.json().await.unwrap();
    let answer = query(body["redirect_to"].as_str().unwrap());
    assert_eq!(answer["error"], "access_denied");
    assert_eq!(answer["iss"], PUBLIC_URL);
    assert_eq!(answer["state"], "xyz");
    assert_eq!(env.consent(&pending).await.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_code_works_once_for_its_client_redirect_and_verifier_and_a_replay_revokes_the_grant(
    pool: PgPool,
) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;
    let pending_for = || async { env.pending(&Authorize::new(&client_id, MCP_RESOURCE)).await };

    // Wrong verifier, wrong redirect and wrong client each fail and spend the code.
    let wrong_verifier = env.approved_code(&pending_for().await).await;
    let response = env
        .token_request(&[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", &wrong_verifier),
            ("redirect_uri", CALLBACK),
            ("code_verifier", &"a".repeat(43)),
        ])
        .await;
    assert_eq!(env.error_code(response).await.1, "invalid_grant");
    let response = env.exchange(&client_id, &wrong_verifier).await;
    assert_eq!(env.error_code(response).await.1, "invalid_grant");

    let wrong_redirect = env.approved_code(&pending_for().await).await;
    let response = env
        .token_request(&[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", &wrong_redirect),
            ("redirect_uri", "http://127.0.0.1:9/callback"),
            ("code_verifier", VERIFIER),
        ])
        .await;
    assert_eq!(env.error_code(response).await.1, "invalid_grant");

    // A resource the code was not issued for.
    let foreign = env.approved_code(&pending_for().await).await;
    let response = env
        .token_request(&[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", &foreign),
            ("redirect_uri", CALLBACK),
            ("code_verifier", VERIFIER),
            ("resource", "https://other.example"),
        ])
        .await;
    assert_eq!(env.error_code(response).await.1, "invalid_target");

    // A good exchange, then the same code again: the Grant it made is revoked.
    let code = env.approved_code(&pending_for().await).await;
    let issued: Value = env.exchange(&client_id, &code).await.json().await.unwrap();
    let access = issued["access_token"].as_str().unwrap();
    let replay = env.exchange(&client_id, &code).await;
    assert_eq!(env.error_code(replay).await.1, "invalid_grant");
    let mcp = env
        .http
        .post(env.server.url("/mcp"))
        .bearer_auth(access)
        .send()
        .await
        .unwrap();
    assert_eq!(mcp.status(), StatusCode::UNAUTHORIZED);

    // A code that ran out is refused.
    let late = env.approved_code(&pending_for().await).await;
    env.server.clock.advance(TimeDelta::seconds(61));
    let response = env.exchange(&client_id, &late).await;
    assert_eq!(env.error_code(response).await.1, "invalid_grant");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn access_tokens_last_an_hour_and_refresh_tokens_ninety_days_without_use(pool: PgPool) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;

    let issued = env.sign_in(&client_id, PUBLIC_URL, "agent").await;
    let access = issued["access_token"].as_str().unwrap().to_owned();
    let refresh_one = issued["refresh_token"].as_str().unwrap().to_owned();
    env.server.clock.advance(TimeDelta::seconds(3599));
    assert_eq!(
        env.get_status("/v1/tokens/me", &access).await.0,
        StatusCode::OK
    );
    env.server.clock.advance(TimeDelta::seconds(1));
    assert_eq!(
        env.get_status("/v1/tokens/me", &access).await.0,
        StatusCode::UNAUTHORIZED
    );

    // Refreshing near the end renews the window.
    env.server.clock.advance(TimeDelta::seconds(89 * 24 * 3600));
    let second: Value = env
        .refresh(&client_id, &refresh_one)
        .await
        .json()
        .await
        .unwrap();
    let refresh_two = second["refresh_token"].as_str().unwrap().to_owned();
    env.server.clock.advance(TimeDelta::seconds(90 * 24 * 3600));
    let expired = env.refresh(&client_id, &refresh_two).await;
    assert_eq!(env.error_code(expired).await.1, "invalid_grant");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_refresh_token_is_only_for_its_client_and_resource_and_never_authenticates_a_request(
    pool: PgPool,
) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;
    let issued = env.sign_in(&client_id, MCP_RESOURCE, "agent").await;
    let refresh_value = issued["refresh_token"].as_str().unwrap();

    assert_eq!(
        env.get_status("/v1/tokens/me", refresh_value).await.0,
        StatusCode::UNAUTHORIZED
    );
    let mcp = env
        .http
        .post(env.server.url("/mcp"))
        .bearer_auth(refresh_value)
        .send()
        .await
        .unwrap();
    assert_eq!(mcp.status(), StatusCode::UNAUTHORIZED);

    let other_client = env
        .token_request(&[
            ("grant_type", "refresh_token"),
            ("client_id", "someone-else"),
            ("refresh_token", refresh_value),
        ])
        .await;
    assert_eq!(env.error_code(other_client).await.1, "invalid_grant");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn revoking_a_grant_fails_its_access_and_refresh_tokens_on_the_next_use(pool: PgPool) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;
    let issued = env.sign_in(&client_id, MCP_RESOURCE, "agent").await;
    let access = issued["access_token"].as_str().unwrap().to_owned();
    let refresh_value = issued["refresh_token"].as_str().unwrap().to_owned();
    assert!(
        mcp_tool_names(&env, &access)
            .await
            .contains(&"list_workflows".to_owned())
    );

    // Connected apps: the Grant lists with its client, provenance, Scope and use.
    let grants: Value = env
        .http
        .get(env.server.url("/v1/grants"))
        .bearer_auth(&env.server.admin_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let grant = &grants["grants"][0];
    assert_eq!(grant["client_name"], "Test Client");
    assert_eq!(grant["provenance"], "cimd");
    assert_eq!(grant["scope"], "agent");
    assert_eq!(grant["resource"], MCP_RESOURCE);
    assert!(grant["last_used_at"].is_string());
    assert!(grant["revoked_at"].is_null());
    let grant_id = grant["id"].as_str().unwrap().to_owned();
    let tokens = env.server.client.list_tokens().await.unwrap();
    let oauth: Vec<_> = tokens.iter().filter(|t| t.grant_id.is_some()).collect();
    assert_eq!(oauth.len(), 1);
    assert_eq!(oauth[0].grant_id.unwrap().to_string(), grant_id);

    let revoked = env
        .http
        .post(env.server.url(&format!("/v1/grants/{grant_id}/revoke")))
        .bearer_auth(&env.server.admin_token)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::OK);
    let mcp = env
        .http
        .post(env.server.url("/mcp"))
        .bearer_auth(&access)
        .send()
        .await
        .unwrap();
    assert_eq!(mcp.status(), StatusCode::UNAUTHORIZED);
    let refreshed = env.refresh(&client_id, &refresh_value).await;
    assert_eq!(env.error_code(refreshed).await.1, "invalid_grant");

    let unknown = env
        .http
        .post(
            env.server
                .url(&format!("/v1/grants/{}/revoke", uuid::Uuid::nil())),
        )
        .bearer_auth(&env.server.admin_token)
        .send()
        .await
        .unwrap();
    assert_eq!(env.error_code(unknown).await.1, "grant_not_found");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_client_can_revoke_its_own_grant_with_either_token_and_no_one_elses(pool: PgPool) {
    let env = env(pool).await;
    let client_id = env.publish_default_client().await;
    let issued = env.sign_in(&client_id, MCP_RESOURCE, "agent").await;
    let refresh_value = issued["refresh_token"].as_str().unwrap().to_owned();
    let revoke = |value: String, client: String| {
        let http = env.http.clone();
        let url = env.server.url("/oauth/revoke");
        async move {
            http.post(url)
                .form(&[("token", value), ("client_id", client)])
                .send()
                .await
                .unwrap()
        }
    };

    // Someone else's client id changes nothing, and neither does an unknown value.
    assert_eq!(
        revoke(refresh_value.clone(), "someone-else".into())
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        revoke("bia_nothing".into(), client_id.clone())
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        env.refresh(&client_id, &refresh_value).await.status(),
        StatusCode::OK
    );
    let rotated: Value = env
        .refresh(&client_id, &refresh_value)
        .await
        .json()
        .await
        .unwrap();
    assert!(rotated.get("error").is_some());

    // Revoking with the access token takes the Grant, so the refresh token dies too.
    let issued = env.sign_in(&client_id, MCP_RESOURCE, "agent").await;
    let refresh_value = issued["refresh_token"].as_str().unwrap().to_owned();
    let access = issued["access_token"].as_str().unwrap().to_owned();
    assert_eq!(
        revoke(access.clone(), client_id.clone()).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        env.refresh(&client_id, &refresh_value).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_metadata_document_is_reused_for_as_long_as_its_cache_headers_allow(pool: PgPool) {
    let env = env(pool).await;
    let id = format!("{}/client.json", env.cimd.uri());
    Mock::given(method("GET"))
        .and(path("/client.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("cache-control", "max-age=60")
                .set_body_json(json!({ "client_id": id, "redirect_uris": [CALLBACK] })),
        )
        .expect(2)
        .mount(&env.cimd)
        .await;
    let request = Authorize::new(&id, MCP_RESOURCE);

    env.pending(&request).await;
    env.pending(&request).await;
    env.server.clock.advance(TimeDelta::seconds(61));
    env.pending(&request).await;
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_server_with_no_ui_url_says_so_in_plain_text(pool: PgPool) {
    let env = env_with(pool, config(false)).await;
    let client_id = env.publish_default_client().await;

    let response = env
        .authorize(&Authorize::new(&client_id, MCP_RESOURCE))
        .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    assert!(response.text().await.unwrap().contains("ui_url"));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_scope_the_mcp_endpoint_cannot_serve_is_a_403_that_names_the_scope(pool: PgPool) {
    let env = env(pool).await;
    let webhook = env.server.webhook_token("w", "t").await;

    let response = env
        .http
        .post(env.server.url("/mcp"))
        .bearer_auth(&webhook)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.headers()["www-authenticate"],
        "Bearer error=\"insufficient_scope\", resource_metadata=\"http://bia.test/.well-known/oauth-protected-resource/mcp\", scope=\"agent\""
    );
}
