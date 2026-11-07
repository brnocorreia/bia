//! Browser sign-in: a password starts a Session, a cookie that carries an admin Token, which
//! expires, is checked against cross-site requests, and ends at sign-out or a password change.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use bia_client::{Error, Origin, Scope};
use bia_server::{ApiConfig, CorsOrigins};
use chrono::TimeDelta;
use reqwest::{Method, Response, StatusCode};
use serde_json::json;
use sqlx::PgPool;

mod common;

use common::{PASSWORD, PUBLIC_URL, RecordedWaits, TestServer};

const UI: &str = "https://ui.example.com";
const FOREIGN: &str = "https://evil.example.com";

/// The status and code of a failed call.
fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> (StatusCode, String) {
    match result {
        Err(Error::Api { status, body }) => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    }
}

fn unauthenticated() -> (StatusCode, String) {
    (StatusCode::UNAUTHORIZED, "invalid_token".into())
}

/// A password that is not [`PASSWORD`].
fn wrong_password() -> String {
    format!("{PASSWORD}-but-wrong")
}

async fn sign_in_raw(server: &TestServer, password: &str) -> Response {
    reqwest::Client::new()
        .post(server.url("/v1/session"))
        .json(&json!({ "password": password }))
        .send()
        .await
        .unwrap()
}

/// The `Set-Cookie` header's attributes, lower-cased, without the cookie's own pair.
fn attributes(response: &Response) -> Vec<String> {
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    cookie
        .split(';')
        .skip(1)
        .map(|attribute| attribute.trim().to_lowercase())
        .collect()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn signing_in_with_the_password_sets_the_cookie_with_the_listed_attributes(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let response = sign_in_raw(&server, PASSWORD).await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cookie.starts_with("bia_session=bia_"), "{cookie}");
    let mut attributes = attributes(&response);
    attributes.sort();
    // `Path=/` so `/mcp` and `/hooks` see it too; `Max-Age` is the Session's 90-day ceiling.
    assert_eq!(
        attributes,
        ["httponly", "max-age=7776000", "path=/", "samesite=lax"]
    );
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["session"]["origin"], "session");
    assert_eq!(body["session"]["scope"], "admin");
    assert!(!body.to_string().contains("bia_"), "{body}");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_cookie_is_secure_when_the_public_url_is_https(pool: PgPool) {
    let config = ApiConfig {
        public_url: Some("https://bia.example.com".parse().unwrap()),
        ..ApiConfig::default()
    };
    let server = TestServer::start_with(pool, config).await;

    let response = sign_in_raw(&server, PASSWORD).await;

    let attributes = attributes(&response);
    assert!(attributes.contains(&"secure".to_owned()), "{attributes:?}");
    assert!(!attributes.iter().any(|a| a.starts_with("domain")));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_wrong_password_is_invalid_credentials_and_sets_no_cookie(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let response = sign_in_raw(&server, &wrong_password()).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().get("set-cookie").is_none());
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "invalid_credentials");
    assert!(!body.to_string().contains(&wrong_password()), "{body}");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn signing_in_before_setup_is_invalid_credentials(pool: PgPool) {
    let server = TestServer::without_tokens(pool).await;

    let result = server.anonymous().sign_in(PASSWORD).await;

    assert_eq!(
        failure(result),
        (StatusCode::UNAUTHORIZED, "invalid_credentials".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_cookie_authenticates_like_a_token(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let signed_in = server.anonymous().sign_in(PASSWORD).await.unwrap();

    let me = server
        .client_with_session(&signed_in.session)
        .current_token()
        .await
        .unwrap();

    assert_eq!(me.id, signed_in.token.id);
    assert_eq!((me.scope, me.origin), (Scope::Admin, Origin::Session));
    let manual = server.client.current_token().await.unwrap();
    assert_eq!(manual.origin, Origin::Manual);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_session_unused_for_14_days_is_refused_and_use_keeps_it_alive(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let session = server.session_client();

    for _ in 0..3 {
        server.clock.advance(TimeDelta::days(13));
        session.current_token().await.unwrap();
    }
    server.clock.advance(TimeDelta::days(14));

    assert_eq!(failure(session.current_token().await), unauthenticated());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_session_older_than_90_days_is_refused_however_often_it_is_used(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let session = server.session_client();

    for _ in 0..8 {
        server.clock.advance(TimeDelta::days(10));
        session.current_token().await.unwrap();
    }
    server.clock.advance(TimeDelta::days(9));
    session.current_token().await.unwrap();
    server.clock.advance(TimeDelta::days(1));

    assert_eq!(failure(session.current_token().await), unauthenticated());
    // Manual Tokens do not expire.
    server.client.current_token().await.unwrap();
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_manual_token_in_the_cookie_is_not_a_session(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let result = server
        .client_with_session(&server.admin_token)
        .current_token()
        .await;

    assert_eq!(failure(result), unauthenticated());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_cookie_request_with_an_unsafe_method_needs_an_origin_of_the_server_or_the_ui(
    pool: PgPool,
) {
    let config = ApiConfig {
        ui_url: Some(UI.parse().unwrap()),
        ..ApiConfig::default()
    };
    let server = TestServer::start_with(pool, config).await;
    let post = |origin: Option<&str>, cookie: &str| {
        let mut request = reqwest::Client::new()
            .post(server.url("/v1/tokens"))
            .header("cookie", format!("bia_session={cookie}"))
            .json(&json!({ "name": "ci", "scope": "agent" }));
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        async move { request.send().await.unwrap() }
    };

    for origin in [
        None,
        Some(FOREIGN),
        Some("null"),
        Some("http://bia.test:81"),
    ] {
        let refused = post(origin, &server.session).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{origin:?}");
        let body: serde_json::Value = refused.json().await.unwrap();
        assert_eq!(body["code"], "origin_mismatch", "{origin:?}");
    }
    for origin in [PUBLIC_URL, UI] {
        let created = post(Some(origin), &server.session).await;
        assert_eq!(created.status(), StatusCode::CREATED, "{origin}");
    }
    // Reads need no Origin, and a bearer Token needs none for anything.
    server
        .anonymous()
        .with_session(&server.session)
        .current_token()
        .await
        .unwrap();
    let bearer = reqwest::Client::new()
        .post(server.url("/v1/tokens"))
        .bearer_auth(&server.admin_token)
        .json(&json!({ "name": "ci", "scope": "agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bearer.status(), StatusCode::CREATED);
}

async fn preflight(server: &TestServer, origin: &str) -> Response {
    reqwest::Client::new()
        .request(Method::OPTIONS, server.url("/v1/schema"))
        .header("origin", origin)
        .header("access-control-request-method", "GET")
        .send()
        .await
        .unwrap()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn only_the_ui_origin_may_send_the_cookie_cross_origin(pool: PgPool) {
    let config = ApiConfig {
        ui_url: Some(UI.parse().unwrap()),
        cors_origins: CorsOrigins::parse(["https://tools.example.com"]).unwrap(),
        ..ApiConfig::default()
    };
    let server = TestServer::start_with(pool, config).await;

    let ui = preflight(&server, UI).await;
    assert_eq!(ui.status(), StatusCode::OK);
    assert_eq!(ui.headers()["access-control-allow-origin"], UI);
    assert_eq!(ui.headers()["access-control-allow-credentials"], "true");

    let listed = preflight(&server, "https://tools.example.com").await;
    assert_eq!(
        listed.headers()["access-control-allow-origin"],
        "https://tools.example.com"
    );
    assert!(
        listed
            .headers()
            .get("access-control-allow-credentials")
            .is_none()
    );

    let other = preflight(&server, FOREIGN).await;
    assert!(other.headers().get("access-control-allow-origin").is_none());
    assert!(
        other
            .headers()
            .get("access-control-allow-credentials")
            .is_none()
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn any_origin_stays_credential_less_even_beside_a_ui_url(pool: PgPool) {
    let config = ApiConfig {
        ui_url: Some(UI.parse().unwrap()),
        cors_origins: CorsOrigins::parse(["*"]).unwrap(),
        ..ApiConfig::default()
    };
    let server = TestServer::start_with(pool, config).await;

    let any = preflight(&server, FOREIGN).await;
    assert_eq!(any.headers()["access-control-allow-origin"], FOREIGN);
    assert!(
        any.headers()
            .get("access-control-allow-credentials")
            .is_none()
    );
    let ui = preflight(&server, UI).await;
    assert_eq!(ui.headers()["access-control-allow-credentials"], "true");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_sixth_failed_attempt_waits_and_the_right_password_still_signs_in(pool: PgPool) {
    let waits = Arc::new(RecordedWaits::default());
    let config = ApiConfig {
        sign_in_wait: Some(waits.clone()),
        ..ApiConfig::default()
    };
    let server = TestServer::start_with(pool, config).await;
    let client = server.anonymous();
    let wrong = wrong_password();

    for _ in 0..5 {
        client.sign_in(&wrong).await.unwrap_err();
    }
    assert!(waits.waits().is_empty(), "{:?}", waits.waits());
    client.sign_in(&wrong).await.unwrap_err();
    client.sign_in(&wrong).await.unwrap_err();
    let signed_in = client.sign_in(PASSWORD).await.unwrap();

    let seconds = |n| Duration::from_secs(n);
    assert_eq!(waits.waits(), [seconds(1), seconds(2), seconds(4)]);
    assert_eq!(signed_in.token.origin, Origin::Session);
    // The success cleared the count.
    client.sign_in(&wrong).await.unwrap_err();
    client.sign_in(PASSWORD).await.unwrap();
    assert_eq!(waits.waits().len(), 3);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_password_change_revokes_the_other_sessions_and_keeps_the_callers(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let first = server.anonymous().sign_in(PASSWORD).await.unwrap();
    let second = server.anonymous().sign_in(PASSWORD).await.unwrap();
    let caller = server.client_with_session(&first.session);
    let replacement = format!("{PASSWORD}-changed");

    let changed = caller
        .change_password(PASSWORD, &replacement)
        .await
        .unwrap();

    // The setup Session and the other sign-in.
    assert_eq!(changed.revoked_sessions, 2);
    caller.current_token().await.unwrap();
    assert_eq!(
        failure(server.session_client().current_token().await),
        unauthenticated()
    );
    assert_eq!(
        failure(
            server
                .client_with_session(&second.session)
                .current_token()
                .await
        ),
        unauthenticated()
    );
    server.client.current_token().await.unwrap();
    assert_eq!(
        failure(server.anonymous().sign_in(PASSWORD).await),
        (StatusCode::UNAUTHORIZED, "invalid_credentials".into())
    );
    server.anonymous().sign_in(&replacement).await.unwrap();
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_password_change_refuses_a_wrong_current_password_and_a_short_new_one(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let other = server.anonymous().sign_in(PASSWORD).await.unwrap();
    let caller = server.session_client();

    let wrong = caller
        .change_password(&wrong_password(), "a-long-enough-new-one")
        .await;
    let short = caller.change_password(PASSWORD, "too short").await;

    assert_eq!(
        failure(wrong),
        (StatusCode::UNAUTHORIZED, "invalid_credentials".into())
    );
    assert_eq!(
        failure(short),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            "password_too_short".into()
        )
    );
    server
        .client_with_session(&other.session)
        .current_token()
        .await
        .unwrap();
    server.anonymous().sign_in(PASSWORD).await.unwrap();
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_password_change_needs_an_admin_and_never_quotes_the_passwords(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let agent = server.token(Scope::Agent).await;
    // serde quotes a value of the wrong type in its own errors.
    let not_a_string = u64::MAX - 7;
    let body = json!({ "current": wrong_password(), "new": not_a_string });

    let forbidden = server
        .client_with(&agent)
        .change_password(PASSWORD, "a-long-enough-new-one")
        .await;
    let malformed = reqwest::Client::new()
        .post(server.url("/v1/user/password"))
        .bearer_auth(&server.admin_token)
        .json(&body)
        .send()
        .await
        .unwrap();

    assert_eq!(
        failure(forbidden),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );
    assert!(malformed.status().is_client_error());
    let text = malformed.text().await.unwrap();
    assert!(text.contains("malformed_request"), "{text}");
    assert!(!text.contains(&wrong_password()), "{text}");
    assert!(!text.contains(&not_a_string.to_string()), "{text}");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn signing_out_revokes_the_session_and_clears_the_cookie(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let signed_in = server.anonymous().sign_in(PASSWORD).await.unwrap();
    let session = server.client_with_session(&signed_in.session);

    let response = reqwest::Client::new()
        .delete(server.url("/v1/session"))
        .header("cookie", format!("bia_session={}", signed_in.session))
        .header("origin", PUBLIC_URL)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with("bia_session=;"), "{cookie}");
    assert!(cookie.contains("Max-Age=0"), "{cookie}");
    assert_eq!(failure(session.current_token().await), unauthenticated());
    let tokens = server.client.list_tokens().await.unwrap();
    let revoked = tokens.iter().find(|t| t.id == signed_in.token.id).unwrap();
    assert_eq!(revoked.revoked_at, Some(common::t0()));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn signing_out_needs_a_session_and_a_valid_origin(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let with_token = server.client.sign_out().await;
    let without_origin = server
        .anonymous()
        .with_session(&server.session)
        .sign_out()
        .await;

    assert_eq!(
        failure(with_token),
        (StatusCode::CONFLICT, "not_a_session".into())
    );
    assert_eq!(
        failure(without_origin),
        (StatusCode::FORBIDDEN, "origin_mismatch".into())
    );
    server.session_client().current_token().await.unwrap();
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_mcp_endpoint_ignores_the_cookie(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let response = reqwest::Client::new()
        .post(server.url("/mcp"))
        .header("cookie", format!("bia_session={}", server.session))
        .header("origin", PUBLIC_URL)
        .header("accept", "application/json, text/event-stream")
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_token_list_says_where_each_token_came_from(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let tokens = server.client.list_tokens().await.unwrap();

    let count = |origin| tokens.iter().filter(|t| t.origin == origin).count();
    assert_eq!((count(Origin::Session), count(Origin::Manual)), (1, 1));
    assert_eq!(tokens.len(), 2);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_session_lists_the_browser_it_was_started_from(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let agent = "Mozilla/5.0 (X11; Linux x86_64) Firefox/130.0";

    let response = reqwest::Client::new()
        .post(server.url("/v1/session"))
        .header("user-agent", agent)
        .json(&json!({ "password": PASSWORD }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let tokens = server.client.list_tokens().await.unwrap();
    let described: Vec<_> = tokens
        .iter()
        .filter(|t| t.user_agent.as_deref() == Some(agent))
        .collect();
    assert_eq!(described.len(), 1, "{tokens:?}");
    assert_eq!(described[0].origin, Origin::Session);
    assert!(tokens.iter().all(|t| t.grant_id.is_none()));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn revoking_a_session_makes_its_cookie_fail_on_the_next_request(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let signed_in = server.anonymous().sign_in(PASSWORD).await.unwrap();
    let session = server.client_with_session(&signed_in.session);
    session.current_token().await.unwrap();

    server
        .client
        .revoke_token(signed_in.token.id)
        .await
        .unwrap();

    assert_eq!(failure(session.current_token().await), unauthenticated());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_session_revoking_itself_through_the_token_route_signs_itself_out(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let session = server.session_client();
    let own = session.current_token().await.unwrap();

    let revoked = session.revoke_token(own.id).await.unwrap();

    assert!(revoked.revoked_at.is_some());
    assert_eq!(failure(session.current_token().await), unauthenticated());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn signing_out_the_others_keeps_this_session_and_the_manual_tokens(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let manual = server.token(Scope::Agent).await;
    let other = server.anonymous().sign_in(PASSWORD).await.unwrap();
    let this = server.session_client();

    let revoked = this.sign_out_others().await.unwrap();

    assert_eq!(revoked, 1);
    assert_eq!(
        failure(
            server
                .client_with_session(&other.session)
                .current_token()
                .await
        ),
        unauthenticated()
    );
    server.client_with(&manual).current_token().await.unwrap();
    this.current_token().await.unwrap();
}
