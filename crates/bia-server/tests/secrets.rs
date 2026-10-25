//! Secrets are written and deleted by `admin` only, listed by name by `admin` and `agent`,
//! stored encrypted, and never returned (ADR-0017, ADR-0049).

#![allow(clippy::unwrap_used)]

use bia_client::{Error, Scope};
use bia_server::ApiConfig;
use bia_store::secret::MasterKey;
use reqwest::StatusCode;
use serde_json::Value;
use sqlx::PgPool;

mod common;

use common::TestServer;

const VALUE: &str = "hunter2-correct-horse";

fn config() -> ApiConfig {
    ApiConfig {
        master_key: Some(MasterKey::parse(&"ab".repeat(32)).unwrap()),
        ..ApiConfig::default()
    }
}

fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> (StatusCode, String) {
    match result {
        Err(Error::Api { status, body }) => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_secret_is_stored_encrypted_and_no_route_returns_its_value(pool: PgPool) {
    let server = TestServer::start_with(pool.clone(), config()).await;

    let set = server.client.put_secret("API_TOKEN", VALUE).await.unwrap();
    assert_eq!(set.name, "API_TOKEN");
    assert_eq!(set.created_at, common::t0());

    server.clock.advance(chrono::TimeDelta::minutes(5));
    let replaced = server
        .client
        .put_secret("API_TOKEN", "another-value")
        .await
        .unwrap();
    assert_eq!(replaced.created_at, common::t0());
    assert!(replaced.updated_at > replaced.created_at);

    let listed = server.client.list_secrets().await.unwrap();
    assert_eq!(listed, vec![replaced]);

    let (ciphertext, nonce): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT ciphertext, nonce FROM secrets WHERE name = 'API_TOKEN'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(nonce.len(), 24);
    for plain in [VALUE, "another-value"] {
        assert!(
            !String::from_utf8_lossy(&ciphertext).contains(plain),
            "the ciphertext shows the value"
        );
    }

    for response in [
        raw(&server, "GET", "/v1/secrets").await,
        raw(&server, "PUT", "/v1/secrets/API_TOKEN").await,
    ] {
        assert!(!response.contains("another-value"), "{response}");
        assert!(!response.contains(VALUE), "{response}");
    }

    server.client.delete_secret("API_TOKEN").await.unwrap();
    assert!(server.client.list_secrets().await.unwrap().is_empty());
    assert_eq!(
        failure(server.client.delete_secret("API_TOKEN").await),
        (StatusCode::NOT_FOUND, "secret_not_found".into())
    );
}

/// The raw body of a request with the admin Token; `PUT` sends a value.
async fn raw(server: &TestServer, method: &str, path: &str) -> String {
    let body = serde_json::json!({ "value": "another-value" });
    reqwest::Client::new()
        .request(method.parse().unwrap(), server.url(path))
        .bearer_auth(&server.admin_token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_agent_token_lists_secret_names_but_cannot_write_or_delete(pool: PgPool) {
    let server = TestServer::start_with(pool, config()).await;
    server.client.put_secret("API_TOKEN", VALUE).await.unwrap();
    let agent = server.client_with(&server.token(Scope::Agent).await);

    let names: Vec<_> = agent
        .list_secrets()
        .await
        .unwrap()
        .into_iter()
        .map(|secret| secret.name)
        .collect();
    assert_eq!(names, ["API_TOKEN"]);
    let forbidden = (StatusCode::FORBIDDEN, "insufficient_scope".to_owned());
    assert_eq!(failure(agent.put_secret("OTHER", VALUE).await), forbidden);
    assert_eq!(failure(agent.put_secret("API_TOKEN", "x").await), forbidden);
    assert_eq!(failure(agent.delete_secret("API_TOKEN").await), forbidden);
    assert_eq!(server.client.list_secrets().await.unwrap().len(), 1);

    let webhook = server.client_with(&server.webhook_token("w", "t").await);
    assert_eq!(failure(webhook.list_secrets().await), forbidden);
    assert_eq!(
        failure(server.anonymous().list_secrets().await),
        (StatusCode::UNAUTHORIZED, "missing_token".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn names_and_values_are_validated(pool: PgPool) {
    let server = TestServer::start_with(pool, config()).await;
    let malformed = (StatusCode::BAD_REQUEST, "malformed_request".to_owned());

    for name in ["1ABC", "a-b", "a%20b"] {
        assert_eq!(
            failure(server.client.put_secret(name, VALUE).await),
            malformed,
            "{name}"
        );
    }
    assert_eq!(
        failure(server.client.put_secret("EMPTY", "").await),
        malformed
    );
    assert!(server.client.list_secrets().await.unwrap().is_empty());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_server_without_a_master_key_refuses_to_store_secrets(pool: PgPool) {
    let server = TestServer::start(pool).await;

    assert_eq!(
        failure(server.client.put_secret("API_TOKEN", VALUE).await),
        (StatusCode::SERVICE_UNAVAILABLE, "master_key_missing".into())
    );
    assert!(server.client.list_secrets().await.unwrap().is_empty());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_openapi_document_never_describes_a_secret_value_in_a_response(pool: PgPool) {
    let server = TestServer::start_with(pool, config()).await;
    let document: Value = reqwest::get(server.url("/openapi.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for schema in ["SecretInfo", "SecretList", "SecretDeleted"] {
        let properties = &document["components"]["schemas"][schema]["properties"];
        assert!(properties.get("value").is_none(), "{schema}");
    }
}
