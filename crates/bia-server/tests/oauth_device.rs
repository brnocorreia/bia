//! The built-in `bia-cli` OAuth client (ADR-0068): its loopback-only authorization code flow
//! and the device flow, driven over HTTP with the test Clock.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bia_server::ApiConfig;
use chrono::TimeDelta;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;

mod common;

use common::{PUBLIC_URL, RecordedWaits, TestServer};

const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

struct Env {
    server: TestServer,
    http: reqwest::Client,
}

async fn env(pool: PgPool) -> Env {
    env_with(pool, ApiConfig::default()).await
}

async fn env_with(pool: PgPool, config: ApiConfig) -> Env {
    let config = ApiConfig {
        public_url: Some(PUBLIC_URL.parse().unwrap()),
        ui_url: Some(PUBLIC_URL.parse().unwrap()),
        ..config
    };
    Env {
        server: TestServer::start_with(pool, config).await,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
    }
}

fn query(location: &str) -> HashMap<String, String> {
    Url::parse(location)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

impl Env {
    fn as_person(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("cookie", format!("bia_session={}", self.server.session))
            .header("origin", &self.server.origin)
    }

    async fn post_form(&self, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
        self.http
            .post(self.server.url(path))
            .form(form)
            .send()
            .await
            .unwrap()
    }

    async fn start_device(&self) -> Value {
        let response = self
            .post_form(
                "/oauth/device_authorization",
                &[
                    ("client_id", "bia-cli"),
                    ("scope", "admin"),
                    ("resource", PUBLIC_URL),
                ],
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        response.json().await.unwrap()
    }

    async fn poll(&self, device_code: &str) -> (StatusCode, Value) {
        let response = self
            .post_form(
                "/oauth/token",
                &[
                    ("grant_type", DEVICE_GRANT),
                    ("client_id", "bia-cli"),
                    ("device_code", device_code),
                ],
            )
            .await;
        (
            response.status(),
            response.json().await.unwrap_or(Value::Null),
        )
    }

    async fn lookup(&self, user_code: &str) -> reqwest::Response {
        self.as_person(
            self.http
                .post(self.server.url("/oauth/device/lookup"))
                .json(&json!({ "user_code": user_code })),
        )
        .send()
        .await
        .unwrap()
    }

    async fn answer(&self, id: &str, decision: &str, body: &Value) -> reqwest::Response {
        self.as_person(
            self.http
                .post(self.server.url(&format!("/oauth/device/{id}/{decision}")))
                .json(body),
        )
        .send()
        .await
        .unwrap()
    }

    /// Looks up `user_code` as the signed-in person and returns the authorization's id.
    async fn find(&self, user_code: &str) -> String {
        let response = self.lookup(user_code).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        body["id"].as_str().unwrap().to_owned()
    }

    async fn get_status(&self, bearer: &str) -> StatusCode {
        self.http
            .get(self.server.url("/v1/tokens/me"))
            .bearer_auth(bearer)
            .send()
            .await
            .unwrap()
            .status()
    }

    fn advance(&self, seconds: i64) {
        self.server.clock.advance(TimeDelta::seconds(seconds));
    }
}

fn error_of(polled: &(StatusCode, Value)) -> &str {
    polled.1["error"].as_str().unwrap_or_default()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_cli_signs_in_with_a_loopback_redirect_on_any_port_and_the_consent_says_it_is_built_in(
    pool: PgPool,
) {
    let env = env(pool).await;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    let authorize = |redirect: &str| {
        let params = [
            ("response_type", "code"),
            ("client_id", "bia-cli"),
            ("redirect_uri", redirect),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("resource", PUBLIC_URL),
            ("scope", "admin"),
            ("state", "xyz"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
        env.http
            .get(env.server.url("/oauth/authorize"))
            .query(&params)
            .send()
    };
    let redirect = "http://127.0.0.1:45871/callback";

    let response = authorize(redirect).await.unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    let id = query(&location)["request"].clone();
    let consent: Value = env
        .as_person(
            env.http
                .get(env.server.url(&format!("/oauth/consent/{id}"))),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(consent["client"]["name"], "bia CLI (built in)");
    assert_eq!(consent["client"]["provenance"], "builtin");
    assert_eq!(consent["client"]["verified"], true);
    assert_eq!(consent["requested_scope"], "admin");

    let approved: Value = env
        .as_person(
            env.http
                .post(env.server.url(&format!("/oauth/consent/{id}/approve")))
                .json(&json!({})),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let code = query(approved["redirect_to"].as_str().unwrap())["code"].clone();
    let tokens: Value = env
        .post_form(
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("client_id", "bia-cli"),
                ("code", &code),
                ("redirect_uri", redirect),
                ("code_verifier", VERIFIER),
            ],
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(tokens["scope"], "admin");
    assert_eq!(
        env.get_status(tokens["access_token"].as_str().unwrap())
            .await,
        StatusCode::OK
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_cli_client_is_refused_every_redirect_that_is_not_its_loopback_callback(pool: PgPool) {
    let env = env(pool).await;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    for redirect in [
        "https://attacker.example/callback",
        "http://attacker.example/callback",
        "http://127.0.0.1:4000/elsewhere",
        "http://localhost:4000/callback",
        "myapp://callback",
    ] {
        let params = [
            ("response_type", "code"),
            ("client_id", "bia-cli"),
            ("redirect_uri", redirect),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("resource", PUBLIC_URL),
        ];
        let response = env
            .http
            .get(env.server.url("/oauth/authorize"))
            .query(&params)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{redirect}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["code"], "invalid_redirect_uri", "{redirect}");
    }

    // Registration cannot take the id over or add a redirect to it.
    let registered = env
        .http
        .post(env.server.url("/oauth/register"))
        .json(
            &json!({ "client_name": "bia-cli", "redirect_uris": ["https://attacker.example/cb"] }),
        )
        .send()
        .await
        .unwrap();
    let body: Value = registered.json().await.unwrap();
    assert_ne!(body["client_id"], "bia-cli");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_device_polls_pending_then_slow_down_then_gets_tokens_after_approval_and_only_once(
    pool: PgPool,
) {
    let env = env(pool).await;
    let started = env.start_device().await;
    let device_code = started["device_code"].as_str().unwrap();
    let user_code = started["user_code"].as_str().unwrap();
    assert_eq!(started["verification_uri"], format!("{PUBLIC_URL}/device"));
    assert_eq!(
        started["verification_uri_complete"],
        format!("{PUBLIC_URL}/device?user_code={user_code}")
    );
    assert_eq!(started["expires_in"], 600);
    assert_eq!(started["interval"], 5);
    assert!(device_code.starts_with("bia_device_"), "{device_code}");
    assert_eq!(user_code.len(), 9);

    assert_eq!(
        error_of(&env.poll(device_code).await),
        "authorization_pending"
    );
    env.advance(1);
    assert_eq!(error_of(&env.poll(device_code).await), "slow_down");
    // Slowing down made the interval 10 s from that poll.
    env.advance(9);
    assert_eq!(error_of(&env.poll(device_code).await), "slow_down");
    env.advance(15);
    assert_eq!(
        error_of(&env.poll(device_code).await),
        "authorization_pending"
    );

    let id = env.find(&user_code.to_lowercase().replace('-', " ")).await;
    let approved = env.answer(&id, "approve", &json!({})).await;
    assert_eq!(approved.status(), StatusCode::OK);
    // The user code names one authorization and is spent by the answer.
    assert_eq!(env.lookup(user_code).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        env.answer(&id, "approve", &json!({})).await.status(),
        StatusCode::NOT_FOUND
    );

    env.advance(15);
    let (status, tokens) = env.poll(device_code).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    assert_eq!(tokens["scope"], "admin");
    let access = tokens["access_token"].as_str().unwrap();
    assert_eq!(env.get_status(access).await, StatusCode::OK);

    // The device code is spent; presenting it again revokes the Grant.
    env.advance(15);
    let replay = env.poll(device_code).await;
    assert_eq!(error_of(&replay), "invalid_grant");
    assert_eq!(env.get_status(access).await, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_device_can_be_approved_with_a_narrower_scope_and_never_a_broader_one(pool: PgPool) {
    let env = env(pool).await;
    let started = env
        .post_form(
            "/oauth/device_authorization",
            &[("client_id", "bia-cli"), ("resource", PUBLIC_URL)],
        )
        .await
        .json::<Value>()
        .await
        .unwrap();
    let id = env.find(started["user_code"].as_str().unwrap()).await;

    let broader = env
        .answer(&id, "approve", &json!({ "scope": "admin" }))
        .await;
    assert_eq!(broader.status(), StatusCode::BAD_REQUEST);
    let body: Value = broader.json().await.unwrap();
    assert_eq!(body["code"], "scope_exceeds_request");
    let consent = env
        .as_person(env.http.get(env.server.url(&format!("/oauth/device/{id}"))))
        .send()
        .await
        .unwrap();
    assert_eq!(consent.status(), StatusCode::OK);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_device_authorization_expires_after_ten_minutes(pool: PgPool) {
    let env = env(pool).await;
    let started = env.start_device().await;
    let device_code = started["device_code"].as_str().unwrap();

    env.advance(599);
    assert_eq!(
        error_of(&env.poll(device_code).await),
        "authorization_pending"
    );
    env.advance(1);
    let expired = env.poll(device_code).await;
    assert_eq!(expired.0, StatusCode::BAD_REQUEST);
    assert_eq!(error_of(&expired), "expired_token");
    assert_eq!(
        env.lookup(started["user_code"].as_str().unwrap())
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_approval_not_collected_in_time_expires_too(pool: PgPool) {
    let env = env(pool).await;
    let started = env.start_device().await;
    let id = env.find(started["user_code"].as_str().unwrap()).await;
    env.answer(&id, "approve", &json!({})).await;

    env.advance(601);

    let polled = env.poll(started["device_code"].as_str().unwrap()).await;
    assert_eq!(error_of(&polled), "expired_token");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn denying_a_device_answers_access_denied(pool: PgPool) {
    let env = env(pool).await;
    let started = env.start_device().await;
    let id = env.find(started["user_code"].as_str().unwrap()).await;

    let denied = env.answer(&id, "deny", &json!({})).await;
    assert_eq!(denied.status(), StatusCode::OK);

    env.advance(5);
    let polled = env.poll(started["device_code"].as_str().unwrap()).await;
    assert_eq!(polled.0, StatusCode::BAD_REQUEST);
    assert_eq!(error_of(&polled), "access_denied");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn only_the_built_in_client_may_use_the_device_flow_and_the_device_code_is_its_own(
    pool: PgPool,
) {
    let env = env(pool).await;
    let registered: Value = env
        .http
        .post(env.server.url("/oauth/register"))
        .json(&json!({ "client_name": "Other", "redirect_uris": ["http://127.0.0.1/cb"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other = registered["client_id"].as_str().unwrap();

    for (client_id, error) in [(other, "unauthorized_client"), ("nobody", "invalid_client")] {
        let refused = env
            .post_form(
                "/oauth/device_authorization",
                &[("client_id", client_id), ("resource", PUBLIC_URL)],
            )
            .await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{client_id}");
        let body: Value = refused.json().await.unwrap();
        assert_eq!(body["error"], error, "{client_id}");
    }
    let wrong_resource = env
        .post_form(
            "/oauth/device_authorization",
            &[
                ("client_id", "bia-cli"),
                ("resource", "http://elsewhere.test"),
            ],
        )
        .await;
    let body: Value = wrong_resource.json().await.unwrap();
    assert_eq!(body["error"], "invalid_target");

    let started = env.start_device().await;
    let id = env.find(started["user_code"].as_str().unwrap()).await;
    env.answer(&id, "approve", &json!({})).await;
    env.advance(5);
    let stolen = env
        .post_form(
            "/oauth/token",
            &[
                ("grant_type", DEVICE_GRANT),
                ("client_id", other),
                ("device_code", started["device_code"].as_str().unwrap()),
            ],
        )
        .await;
    let body: Value = stolen.json().await.unwrap();
    assert_eq!(body["error"], "invalid_grant");
    let unknown = env.poll("bia_device_nope").await;
    assert_eq!(error_of(&unknown), "invalid_grant");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn answering_a_device_authorization_needs_a_session_and_the_lookup_is_throttled(
    pool: PgPool,
) {
    let waits = Arc::new(RecordedWaits::default());
    let env = env_with(
        pool,
        ApiConfig {
            sign_in_wait: Some(waits.clone()),
            ..ApiConfig::default()
        },
    )
    .await;
    let started = env.start_device().await;
    let user_code = started["user_code"].as_str().unwrap();

    // An admin Token of another origin cannot approve access for itself.
    let with_token = env
        .http
        .post(env.server.url("/oauth/device/lookup"))
        .bearer_auth(&env.server.admin_token)
        .json(&json!({ "user_code": user_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(with_token.status(), StatusCode::FORBIDDEN);
    let anonymous = env
        .http
        .post(env.server.url("/oauth/device/lookup"))
        .json(&json!({ "user_code": user_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    for _ in 0..5 {
        assert_eq!(
            env.lookup("BBBB-BBBB").await.status(),
            StatusCode::NOT_FOUND
        );
    }
    assert!(waits.waits().is_empty());
    assert_eq!(
        env.lookup("BBBB-BBBB").await.status(),
        StatusCode::NOT_FOUND
    );
    assert!(!waits.waits().is_empty(), "the sixth guess waits");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_server_without_a_ui_url_cannot_start_a_device_flow(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let response = reqwest::Client::new()
        .post(server.url("/oauth/device_authorization"))
        .form(&[("client_id", "bia-cli"), ("resource", PUBLIC_URL)])
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn device_authorizations_are_rate_limited_per_address(pool: PgPool) {
    let env = env(pool).await;
    for _ in 0..20 {
        env.start_device().await;
    }

    let refused = env
        .post_form(
            "/oauth/device_authorization",
            &[("client_id", "bia-cli"), ("resource", PUBLIC_URL)],
        )
        .await;

    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
}
