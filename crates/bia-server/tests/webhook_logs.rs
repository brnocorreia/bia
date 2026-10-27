//! Webhook credentials and HMAC keys never reach the logs.

#![allow(clippy::unwrap_used)]

use std::io;
use std::sync::{Arc, Mutex};

use bia_client::RunStatus;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

mod common;

use common::webhook::{
    HMAC_KEY, SIGNATURE_HEADER, SIGNED, WORKFLOW, config, definition, deploy, post, sign,
};
use common::{TestServer, eventually, spawn_worker};

/// Collects what a `tracing` subscriber writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn webhook_credentials_and_hmac_keys_stay_out_of_the_logs(pool: PgPool) {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    // Global rather than scoped to this thread: a scoped subscriber misses events from
    // callsites whose interest another test thread cached first. This file holds one test.
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let server = TestServer::start_with(pool.clone(), config()).await;
    deploy(&server, &definition()).await;
    let _workers = spawn_worker(&pool, &server.clock, Arc::new(common::registry()));
    let token = server.webhook_token(WORKFLOW, SIGNED).await;
    let payload = json!({ "email": "ada@example.com" }).to_string();
    let signature = sign(payload.as_bytes());

    let accepted = post(&server, WORKFLOW, SIGNED, payload.clone())
        .bearer_auth(&token)
        .header(SIGNATURE_HEADER, &signature)
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let body: Value = accepted.json().await.unwrap();
    let run_id: Uuid = serde_json::from_value(body["run_id"].clone()).unwrap();
    eventually("the run to succeed", || async {
        server.client.get_run(run_id).await.unwrap().summary.status == RunStatus::Succeeded
    })
    .await;
    let tampered = post(&server, WORKFLOW, SIGNED, "{}")
        .bearer_auth(&token)
        .header(SIGNATURE_HEADER, &signature);
    assert_eq!(
        tampered.send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    server.client.delete_secret("SIGNUP_HMAC").await.unwrap();
    let unavailable = post(&server, WORKFLOW, SIGNED, payload.clone())
        .header(SIGNATURE_HEADER, &signature)
        .send()
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("run started by webhook"), "{logs}");
    assert!(logs.contains("SIGNUP_HMAC"), "{logs}");
    for secret in [
        token.as_str(),
        signature.trim_start_matches("sha256="),
        HMAC_KEY,
    ] {
        assert!(!logs.contains(secret), "{secret} in the logs");
    }
}
