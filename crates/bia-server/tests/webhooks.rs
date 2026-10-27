//! Webhook Triggers start Runs of the Active Version from `/hooks/{workflow_id}/{trigger_id}`,
//! authenticated by the Trigger's bound Token or its HMAC signature (ADR-0058).

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use bia_client::{RunQuery, RunStatus, Scope};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

mod common;

use common::webhook::{
    HMAC_KEY, PLAIN, SIGNATURE_HEADER, SIGNED, WORKFLOW, config, definition, deploy, post, sign,
};
use common::{TestServer, eventually, spawn_worker};

async fn server(pool: PgPool) -> TestServer {
    let server = TestServer::start_with(pool, config()).await;
    deploy(&server, &definition()).await;
    server
}

/// The status and error `code` of `response`, or its `run_id` on success.
async fn outcome(response: reqwest::RequestBuilder) -> (StatusCode, Value) {
    let response = response.send().await.unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    let detail = body.get("code").cloned().unwrap_or(body["run_id"].clone());
    (status, detail)
}

async fn run_count(server: &TestServer) -> usize {
    server
        .client
        .list_runs(&RunQuery::default())
        .await
        .unwrap()
        .runs
        .len()
}

fn code(code: &str) -> Value {
    Value::String(code.to_owned())
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_request_with_the_bound_token_starts_a_run_whose_input_is_the_payload(pool: PgPool) {
    let server = server(pool).await;
    let token = server.webhook_token(WORKFLOW, PLAIN).await;
    let payload = json!({ "email": "ada@example.com", "plan": "vip" });

    let response = post(&server, WORKFLOW, PLAIN, payload.to_string())
        .bearer_auth(&token)
        .header("X-GitHub-Event", "push")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body.as_object().unwrap().len(), 1, "{body}");
    let run_id: Uuid = serde_json::from_value(body["run_id"].clone()).unwrap();
    let run = server.client.get_run(run_id).await.unwrap();
    assert_eq!(run.input, payload);
    assert_eq!(run.summary.workflow_id, WORKFLOW);
    assert_eq!(run.summary.status, RunStatus::Pending);
    let trigger = &run.summary.trigger;
    assert_eq!(trigger["type"], "webhook");
    assert_eq!(trigger["id"], PLAIN);
    assert_eq!(trigger["received_at"], json!(common::t0()));
    assert_eq!(trigger["headers"]["x-github-event"], "push");
    assert_eq!(trigger["headers"]["content-type"], "application/json");
    assert!(trigger["headers"].get("authorization").is_none());

    let by_admin =
        post(&server, WORKFLOW, PLAIN, payload.to_string()).bearer_auth(&server.admin_token);
    assert_eq!(outcome(by_admin).await.0, StatusCode::ACCEPTED);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_empty_body_is_an_empty_object_input(pool: PgPool) {
    let server = TestServer::start_with(pool, config()).await;
    let mut open = definition();
    open.as_object_mut().unwrap().remove("input_schema");
    deploy(&server, &open).await;
    let token = server.webhook_token(WORKFLOW, PLAIN).await;

    let (status, run_id) = outcome(post(&server, WORKFLOW, PLAIN, "").bearer_auth(&token)).await;

    assert_eq!(status, StatusCode::ACCEPTED);
    let run = server
        .client
        .get_run(serde_json::from_value(run_id).unwrap())
        .await
        .unwrap();
    assert_eq!(run.input, json!({}));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_missing_or_wrong_credential_is_refused_and_starts_nothing(pool: PgPool) {
    let server = server(pool).await;
    let payload = json!({ "email": "ada@example.com" }).to_string();
    let agent = server.token(Scope::Agent).await;
    let elsewhere = server.webhook_token(WORKFLOW, SIGNED).await;
    let revoked = server
        .client
        .create_token(&bia_client::NewToken {
            name: "revoked".into(),
            scope: Scope::Webhook,
            webhook: Some(bia_client::WebhookBinding {
                workflow_id: WORKFLOW.into(),
                trigger_id: PLAIN.into(),
            }),
        })
        .await
        .unwrap();
    server.client.revoke_token(revoked.token.id).await.unwrap();
    let unknown = format!("bia_{}", "0".repeat(64));

    let request = || post(&server, WORKFLOW, PLAIN, payload.clone());
    assert_eq!(
        outcome(request()).await,
        (StatusCode::UNAUTHORIZED, code("missing_token"))
    );
    for token in [unknown.as_str(), revoked.value.as_str(), "not-a-token"] {
        assert_eq!(
            outcome(request().bearer_auth(token)).await,
            (StatusCode::UNAUTHORIZED, code("invalid_token")),
            "{token}"
        );
    }
    for token in [&agent, &elsewhere] {
        assert_eq!(
            outcome(request().bearer_auth(token)).await,
            (StatusCode::FORBIDDEN, code("insufficient_scope"))
        );
    }
    // A signature does not stand in for the Token of a Trigger without `hmac`.
    let signed = request().header(SIGNATURE_HEADER, sign(payload.as_bytes()));
    assert_eq!(
        outcome(signed).await,
        (StatusCode::UNAUTHORIZED, code("missing_token"))
    );
    assert_eq!(run_count(&server).await, 0);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_signed_request_is_accepted_and_a_tampered_or_unsigned_one_is_not(pool: PgPool) {
    let server = server(pool).await;
    let payload = json!({ "email": "ada@example.com" }).to_string();
    let signature = sign(payload.as_bytes());
    let request = |body: &str| post(&server, WORKFLOW, SIGNED, body.to_owned());

    let tampered = json!({ "email": "eve@example.com" }).to_string();
    assert_eq!(
        outcome(request(&tampered).header(SIGNATURE_HEADER, &signature)).await,
        (StatusCode::UNAUTHORIZED, code("invalid_signature"))
    );
    for garbage in ["sha256=zz", "", "sha1=00"] {
        assert_eq!(
            outcome(request(&payload).header(SIGNATURE_HEADER, garbage)).await,
            (StatusCode::UNAUTHORIZED, code("invalid_signature")),
            "{garbage}"
        );
    }
    assert_eq!(
        outcome(request(&payload)).await,
        (StatusCode::UNAUTHORIZED, code("missing_signature"))
    );
    // A Token, even the bound one, does not stand in for the signature.
    let token = server.webhook_token(WORKFLOW, SIGNED).await;
    assert_eq!(
        outcome(request(&payload).bearer_auth(&token)).await,
        (StatusCode::UNAUTHORIZED, code("missing_signature"))
    );
    assert_eq!(run_count(&server).await, 0);

    let accepted = outcome(request(&payload).header(SIGNATURE_HEADER, &signature)).await;
    assert_eq!(accepted.0, StatusCode::ACCEPTED);
    let bare_upper_hex = signature.trim_start_matches("sha256=").to_uppercase();
    let accepted = outcome(request(&payload).header(SIGNATURE_HEADER, bare_upper_hex)).await;
    assert_eq!(accepted.0, StatusCode::ACCEPTED);
    let with_token = request(&payload)
        .header(SIGNATURE_HEADER, &signature)
        .bearer_auth(&token);
    assert_eq!(outcome(with_token).await.0, StatusCode::ACCEPTED);
    // A wrong Token is refused rather than ignored.
    let wrong_token = request(&payload)
        .header(SIGNATURE_HEADER, &signature)
        .bearer_auth(server.webhook_token(WORKFLOW, PLAIN).await);
    assert_eq!(
        outcome(wrong_token).await,
        (StatusCode::FORBIDDEN, code("insufficient_scope"))
    );
    assert_eq!(run_count(&server).await, 3);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_missing_hmac_secret_is_reported_as_unavailable_without_starting_a_run(pool: PgPool) {
    let server = server(pool).await;
    server.client.delete_secret("SIGNUP_HMAC").await.unwrap();
    let payload = json!({ "email": "ada@example.com" }).to_string();

    let response = post(&server, WORKFLOW, SIGNED, payload.clone())
        .header(SIGNATURE_HEADER, sign(payload.as_bytes()))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "hmac_secret_unavailable");
    assert!(!body["message"].as_str().unwrap().contains("SIGNUP_HMAC"));
    assert_eq!(run_count(&server).await, 0);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_payload_that_breaks_the_input_schema_or_is_not_json_is_rejected(pool: PgPool) {
    let server = server(pool).await;
    let token = server.webhook_token(WORKFLOW, PLAIN).await;

    let response = post(
        &server,
        WORKFLOW,
        PLAIN,
        json!({ "plan": "vip" }).to_string(),
    )
    .bearer_auth(&token)
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "invalid_run_input");
    assert!(
        body["message"].as_str().unwrap().contains("email"),
        "{body}"
    );

    let not_json = post(&server, WORKFLOW, PLAIN, "email=ada@example.com").bearer_auth(&token);
    assert_eq!(
        outcome(not_json).await,
        (StatusCode::BAD_REQUEST, code("malformed_request"))
    );
    assert_eq!(run_count(&server).await, 0);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_webhook_token_reaches_no_other_route(pool: PgPool) {
    let server = server(pool).await;
    let client = server.client_with(&server.webhook_token(WORKFLOW, PLAIN).await);
    let forbidden = (StatusCode::FORBIDDEN, "insufficient_scope".to_owned());
    let failure = |error: bia_client::Error| match error {
        bia_client::Error::Api { status, body } => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    };

    assert_eq!(failure(client.schema().await.unwrap_err()), forbidden);
    assert_eq!(failure(client.list_tokens().await.unwrap_err()), forbidden);
    assert_eq!(
        failure(client.list_runs(&RunQuery::default()).await.unwrap_err()),
        forbidden
    );
    let input = json!({ "email": "a@b" });
    let manual = client.trigger_run(WORKFLOW, &input, None).await;
    assert_eq!(failure(manual.unwrap_err()), forbidden);
    assert_eq!(
        failure(client.list_workflows().await.unwrap_err()),
        forbidden
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn only_webhook_triggers_of_the_active_version_respond(pool: PgPool) {
    let server = server(pool).await;
    // Version 2 adds a webhook Trigger but is only Published, not Active.
    let mut next = definition();
    next["triggers"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "later", "type": "webhook" }));
    let admin = &server.client;
    let draft = admin.create_draft(WORKFLOW, &next).await.unwrap();
    admin
        .publish_version(WORKFLOW, draft.version.summary.number)
        .await
        .unwrap();
    // A Workflow with only a Draft has no Active Version.
    let mut drafted = definition();
    drafted["id"] = json!("drafted");
    admin.create_draft("drafted", &drafted).await.unwrap();
    let payload = json!({ "email": "ada@example.com" }).to_string();

    for (workflow, trigger) in [
        (WORKFLOW, "later"),
        (WORKFLOW, "by_hand"),
        (WORKFLOW, "nightly"),
        ("drafted", PLAIN),
        ("nowhere", PLAIN),
        ("Not A Workflow", PLAIN),
    ] {
        let request = || post(&server, workflow, trigger, payload.clone());
        assert_eq!(
            outcome(request().bearer_auth(&server.admin_token)).await,
            (StatusCode::NOT_FOUND, code("webhook_not_found")),
            "{workflow}/{trigger}"
        );
        // Without a Token, a missing Trigger looks like one that needs a Token.
        assert_eq!(
            outcome(request()).await,
            (StatusCode::UNAUTHORIZED, code("missing_token")),
            "{workflow}/{trigger}"
        );
    }
    assert_eq!(run_count(&server).await, 0);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn credentials_and_the_hmac_key_stay_out_of_the_trigger_and_the_timeline(pool: PgPool) {
    let server = server(pool.clone()).await;
    let _workers = spawn_worker(&pool, &server.clock, Arc::new(common::registry()));
    let token = server.webhook_token(WORKFLOW, SIGNED).await;
    let payload = json!({ "email": "ada@example.com" }).to_string();
    let signature = sign(payload.as_bytes());

    let (status, run_id) = outcome(
        post(&server, WORKFLOW, SIGNED, payload.clone())
            .bearer_auth(&token)
            .header(SIGNATURE_HEADER, &signature)
            .header("Cookie", "session=s3cr3t-cookie")
            .header("X-Api-Key", "s3cr3t-api-key")
            .header("X-Auth-Token", "s3cr3t-auth-token")
            .header("X-Request-Id", "req-1"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let run_id: Uuid = serde_json::from_value(run_id).unwrap();
    eventually("the run to succeed", || async {
        server.client.get_run(run_id).await.unwrap().summary.status == RunStatus::Succeeded
    })
    .await;

    let run = server.client.get_run(run_id).await.unwrap();
    let headers = run.summary.trigger["headers"].as_object().unwrap();
    let mut names: Vec<&str> = headers.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert!(names.contains(&"x-request-id"), "{names:?}");
    for dropped in [
        "authorization",
        "cookie",
        "x-api-key",
        "x-auth-token",
        "x-hub-signature-256",
    ] {
        assert!(!names.contains(&dropped), "{dropped} in {names:?}");
    }
    let echoed = &run.timeline.last().unwrap().output.as_ref().unwrap()["trigger"];
    assert_eq!(echoed, &run.summary.trigger);
    let everything = serde_json::to_string(&run).unwrap();
    for secret in [
        token.as_str(),
        signature.as_str(),
        signature.trim_start_matches("sha256="),
        HMAC_KEY,
        "s3cr3t",
    ] {
        assert!(!everything.contains(secret), "{secret} in {everything}");
    }
}
