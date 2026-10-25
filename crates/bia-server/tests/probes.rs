//! `GET /health` answers whenever the process serves; `GET /ready` only once the database is
//! reachable and migrated.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use bia_client::Error;
use reqwest::StatusCode;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

mod common;

use common::TestServer;

fn not_ready(error: Error) -> String {
    match error {
        Error::Api { status, body } => {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body.code, "not_ready");
            body.message
        }
        other => panic!("expected an API error, got {other:?}"),
    }
}

/// A pool whose database never answers: nothing listens on port 1.
fn unreachable_pool() -> PgPool {
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://bia:bia@127.0.0.1:1/bia")
        .unwrap()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_migrated_database_is_ready(pool: PgPool) {
    let server = TestServer::start(pool).await;

    assert_eq!(server.client.health().await.unwrap().status, "ok");
    assert_eq!(server.client.ready().await.unwrap().status, "ready");
}

#[sqlx::test(migrations = false)]
async fn a_database_with_pending_migrations_is_not_ready(pool: PgPool) {
    let server = TestServer::start(pool.clone()).await;

    let message = not_ready(server.client.ready().await.unwrap_err());
    assert!(message.contains("bia migrate"), "{message}");

    bia_store::migrate(&pool).await.unwrap();
    assert_eq!(server.client.ready().await.unwrap().status, "ready");
}

#[tokio::test]
async fn an_unreachable_database_is_not_ready_but_the_process_is_healthy() {
    let server = TestServer::start(unreachable_pool()).await;

    let message = not_ready(server.client.ready().await.unwrap_err());
    assert!(message.contains("unreachable"), "{message}");
    assert_eq!(server.client.health().await.unwrap().status, "ok");
}
