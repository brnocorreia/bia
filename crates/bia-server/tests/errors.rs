//! Every failure, from an unknown route to an invalid Definition, has the same error body.

#![allow(clippy::unwrap_used)]

use axum::routing::post;
use bia_core::clock::ManualClock;
use bia_core::validation::{KnownResources, Target, Validator};
use bia_server::{ApiConfig, ApiError, Json};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

use common::TestServer;

async fn error(response: reqwest::Response, status: StatusCode, code: &str) -> Value {
    assert_eq!(response.status(), status);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], code, "{body}");
    assert!(body["message"].is_string(), "{body}");
    body
}

/// The server plus `POST /test/validate`, which validates its body the way Definition
/// endpoints will and answers with `invalid_definition` when it has errors.
async fn server_with_a_validating_route(pool: PgPool) -> TestServer {
    async fn validate(Json(definition): Json<Value>) -> Result<Json<Value>, ApiError> {
        let actions = common::registry();
        let report = Validator::new(Target::Draft, &actions, &KnownResources::new())
            .validate_value(definition);
        if report.has_errors() {
            return Err(ApiError::invalid_definition(report.diagnostics));
        }
        Ok(Json(json!({ "valid": true })))
    }
    let clock = ManualClock::new(common::t0());
    let state = common::state(pool, &clock, ApiConfig::default());
    let router = bia_server::router(state).route("/test/validate", post(validate));
    TestServer::serve(router, clock).await
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_unknown_route_is_route_not_found(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let response = reqwest::get(server.url("/v1/nothing-here")).await.unwrap();

    let body = error(response, StatusCode::NOT_FOUND, "route_not_found").await;
    assert!(body.get("diagnostics").is_none(), "{body}");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_known_route_with_another_method_is_method_not_allowed(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let response = reqwest::Client::new()
        .delete(server.url("/v1/schema"))
        .send()
        .await
        .unwrap();

    error(
        response,
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
    )
    .await;
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_body_that_is_not_json_is_a_malformed_request(pool: PgPool) {
    let server = server_with_a_validating_route(pool).await;
    let http = reqwest::Client::new();

    let not_json = http
        .post(server.url("/test/validate"))
        .header("content-type", "application/json")
        .body("{ \"schema_version\": ")
        .send()
        .await
        .unwrap();
    error(not_json, StatusCode::BAD_REQUEST, "malformed_request").await;

    let wrong_type = http
        .post(server.url("/test/validate"))
        .header("content-type", "text/yaml")
        .body("schema_version: 1")
        .send()
        .await
        .unwrap();
    error(
        wrong_type,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "malformed_request",
    )
    .await;
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_invalid_definition_carries_its_diagnostics(pool: PgPool) {
    let server = server_with_a_validating_route(pool).await;
    let http = reqwest::Client::new();

    let response = http
        .post(server.url("/test/validate"))
        .json(&json!({
            "schema_version": 1,
            "id": "greet",
            "steps": [{ "id": "hello", "action": "test.ecko", "params": {} }],
        }))
        .send()
        .await
        .unwrap();

    let body = error(
        response,
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_definition",
    )
    .await;
    let diagnostics = body["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1, "{body}");
    assert_eq!(diagnostics[0]["code"], "unknown_action");
    assert_eq!(diagnostics[0]["path"], "/steps/0/action");
    assert_eq!(diagnostics[0]["severity"], "error");
    assert!(
        diagnostics[0]["suggestion"]
            .as_str()
            .unwrap()
            .contains("test.echo"),
        "{body}"
    );
}
