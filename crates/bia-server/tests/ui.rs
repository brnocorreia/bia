//! The embedded UI (`ui` feature): screens by deep link, never in the API's way (ADR-0052).
//! These pass on the placeholder page a checkout without `ui/dist` embeds, and on the real bundle.

#![cfg(feature = "ui")]
#![allow(clippy::unwrap_used)]

mod common;

use common::TestServer;
use reqwest::StatusCode;
use reqwest::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use sqlx::PgPool;

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_deep_link_loads_the_index_page(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let http = reqwest::Client::new();

    let root = http.get(server.url("/")).send().await.unwrap();
    assert_eq!(root.status(), StatusCode::OK);
    assert!(
        root.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(root.headers()[CACHE_CONTROL], "no-cache");
    assert!(
        root.headers()[CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("script-src 'self'")
    );
    let index = root.text().await.unwrap();

    for path in ["/runs/123", "/workflows", "/login"] {
        let page = http.get(server.url(path)).send().await.unwrap();
        assert_eq!(page.status(), StatusCode::OK, "{path}");
        assert_eq!(page.text().await.unwrap(), index, "{path}");
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_api_is_never_shadowed_by_the_fallback(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let http = reqwest::Client::new();

    for path in ["/health", "/ready", "/openapi.json"] {
        let response = http.get(server.url(path)).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(
            !response.headers()[CONTENT_TYPE]
                .to_str()
                .unwrap()
                .contains("html"),
            "{path}"
        );
    }

    let workflows = http.get(server.url("/v1/workflows")).send().await.unwrap();
    assert_eq!(workflows.status(), StatusCode::UNAUTHORIZED);

    // Unknown API paths keep the uniform error body instead of the index page.
    for path in ["/v1/nope", "/v1", "/mcp/nope"] {
        let response = http.get(server.url(path)).send().await.unwrap();
        assert!(
            response.status().is_client_error(),
            "{path}: {}",
            response.status()
        );
        assert!(
            !response.headers()[CONTENT_TYPE]
                .to_str()
                .unwrap()
                .contains("html"),
            "{path}"
        );
    }

    // Only reads reach the UI.
    let post = http.post(server.url("/runs/123")).send().await.unwrap();
    assert_eq!(post.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_missing_file_is_not_found_instead_of_the_index_page(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let response = reqwest::get(server.url("/assets/gone.js")).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn config_points_the_ui_at_its_own_origin(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let response = reqwest::get(server.url("/config.json")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(response.headers()[CACHE_CONTROL], "no-cache");
    let config: serde_json::Value = response.json().await.unwrap();
    assert_eq!(config, serde_json::json!({ "apiUrl": "" }));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_unchanged_page_answers_not_modified(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let http = reqwest::Client::new();
    let first = http.get(server.url("/")).send().await.unwrap();
    let etag = first.headers()[ETAG].clone();

    let again = http
        .get(server.url("/"))
        .header(IF_NONE_MATCH, etag)
        .send()
        .await
        .unwrap();

    assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
}
