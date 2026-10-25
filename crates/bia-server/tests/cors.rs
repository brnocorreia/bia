//! Browsers may call the API from the configured CORS origins, and from no others.

#![allow(clippy::unwrap_used)]

use bia_server::{ApiConfig, CorsOrigins};
use reqwest::{Method, Response, StatusCode};
use sqlx::PgPool;

mod common;

use common::TestServer;

const UI: &str = "https://ui.example.com";

async fn preflight(server: &TestServer, origin: &str) -> Response {
    reqwest::Client::new()
        .request(Method::OPTIONS, server.url("/v1/schema"))
        .header("origin", origin)
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "authorization")
        .send()
        .await
        .unwrap()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_preflight_from_a_configured_origin_succeeds_and_one_from_another_fails(pool: PgPool) {
    let config = ApiConfig {
        cors_origins: CorsOrigins::parse([UI, "http://localhost:5173"]).unwrap(),
        ..ApiConfig::default()
    };
    let server = TestServer::start_with(pool, config).await;

    let allowed = preflight(&server, UI).await;
    assert_eq!(allowed.status(), StatusCode::OK);
    let headers = allowed.headers();
    assert_eq!(headers["access-control-allow-origin"], UI);
    assert!(
        headers["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .contains("authorization"),
        "{headers:?}"
    );
    assert!(
        headers["access-control-allow-methods"]
            .to_str()
            .unwrap()
            .contains("GET")
    );

    let refused = preflight(&server, "https://evil.example.com").await;
    assert!(
        refused
            .headers()
            .get("access-control-allow-origin")
            .is_none(),
        "{:?}",
        refused.headers()
    );

    let request = reqwest::Client::new()
        .get(server.url("/v1/schema"))
        .header("origin", UI)
        .bearer_auth(&server.admin_token)
        .send()
        .await
        .unwrap();
    assert_eq!(request.status(), StatusCode::OK);
    assert_eq!(request.headers()["access-control-allow-origin"], UI);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn without_configured_origins_no_preflight_succeeds(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let refused = preflight(&server, UI).await;

    assert!(
        refused
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
}
