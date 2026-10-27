//! Workflows and Versions through the API: Drafts are authored and validated by `agent`
//! Tokens, while publishing and activating belong to `admin` (ADR-0008, ADR-0044).

#![allow(clippy::unwrap_used)]

use bia_client::{Error, Scope, Target, VersionState};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

use common::TestServer;

fn definition(id: &str, greeting: &str) -> Value {
    json!({
        "schema_version": 1,
        "id": id,
        "steps": [{ "id": "hello", "action": "test.echo", "params": { "greeting": greeting } }],
    })
}

fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> (StatusCode, String) {
    match result {
        Err(Error::Api { status, body }) => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_agent_creates_replaces_and_validates_a_draft(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let agent = server.client_with(&server.token(Scope::Agent).await);

    let created = agent
        .create_draft("greeter", &definition("greeter", "hi"))
        .await
        .unwrap();
    assert_eq!(created.version.summary.number, 1);
    assert_eq!(created.version.summary.state, VersionState::Draft);
    assert!(created.diagnostics.is_empty());

    // Drafts may be saved with errors; the Diagnostics say where.
    let broken = json!({
        "schema_version": 1,
        "id": "greeter",
        "steps": [{ "id": "hello", "action": "test.echo", "params": { "greeting": 1 } }],
    });
    let replaced = agent.replace_draft("greeter", 1, &broken).await.unwrap();
    assert_eq!(replaced.version.summary.number, 1);
    assert_eq!(replaced.version.definition, broken);
    assert!(replaced.diagnostics.iter().any(|d| d.is_error()));

    let report = agent
        .validate_version("greeter", 1, Target::Draft)
        .await
        .unwrap();
    assert!(!report.valid);
    let diagnostic = report.diagnostics.iter().find(|d| d.is_error()).unwrap();
    assert!(diagnostic.path.starts_with("/steps/0/params"));

    let adhoc = agent
        .validate_definition(&definition("greeter", "hi"), Target::Publish)
        .await
        .unwrap();
    assert!(adhoc.valid);

    let workflows = agent.list_workflows().await.unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].active_version, None);
    assert_eq!(agent.list_versions("greeter").await.unwrap().len(), 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn drafts_are_numbered_per_workflow(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let client = &server.client;

    client
        .create_draft("a", &definition("a", "1"))
        .await
        .unwrap();
    let second = client
        .create_draft("a", &definition("a", "2"))
        .await
        .unwrap();
    let other = client
        .create_draft("b", &definition("b", "1"))
        .await
        .unwrap();

    assert_eq!(second.version.summary.number, 2);
    assert_eq!(other.version.summary.number, 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn publishing_and_activating_need_an_admin_token(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let agent = server.client_with(&server.token(Scope::Agent).await);
    agent
        .create_draft("greeter", &definition("greeter", "hi"))
        .await
        .unwrap();

    assert_eq!(
        failure(agent.publish_version("greeter", 1).await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );
    server.client.publish_version("greeter", 1).await.unwrap();
    assert_eq!(
        failure(agent.activate_version("greeter", 1).await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );
    let workflow = server.client.activate_version("greeter", 1).await.unwrap();
    assert_eq!(workflow.active_version, Some(1));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn publishing_a_draft_with_errors_fails_with_its_diagnostics(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let unknown_action = json!({
        "schema_version": 1,
        "id": "greeter",
        "steps": [{ "id": "hello", "action": "test.shell", "params": {} }],
    });
    server
        .client
        .create_draft("greeter", &unknown_action)
        .await
        .unwrap();

    let Err(Error::Api { status, body }) = server.client.publish_version("greeter", 1).await else {
        panic!("publishing should fail");
    };
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body.code, "invalid_definition");
    let diagnostic = body.diagnostics.iter().find(|d| d.is_error()).unwrap();
    assert_eq!(diagnostic.path, "/steps/0/action");
    // Still a Draft.
    let version = server.client.get_version("greeter", 1).await.unwrap();
    assert_eq!(version.summary.state, VersionState::Draft);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_published_version_is_immutable(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let client = &server.client;
    client
        .create_draft("greeter", &definition("greeter", "hi"))
        .await
        .unwrap();

    let published = client.publish_version("greeter", 1).await.unwrap();
    assert_eq!(published.version.summary.state, VersionState::Published);
    assert!(published.version.summary.published_at.is_some());

    assert_eq!(
        failure(
            client
                .replace_draft("greeter", 1, &definition("greeter", "bye"))
                .await
        ),
        (StatusCode::CONFLICT, "version_published".into())
    );
    assert_eq!(
        failure(client.publish_version("greeter", 1).await),
        (StatusCode::CONFLICT, "version_published".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn activating_an_older_version_rolls_back(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let client = &server.client;
    for greeting in ["one", "two"] {
        let draft = client
            .create_draft("greeter", &definition("greeter", greeting))
            .await
            .unwrap();
        client
            .publish_version("greeter", draft.version.summary.number)
            .await
            .unwrap();
    }

    client.activate_version("greeter", 2).await.unwrap();
    assert_eq!(
        client.get_workflow("greeter").await.unwrap().active_version,
        Some(2)
    );

    client.activate_version("greeter", 1).await.unwrap();
    assert_eq!(
        client.get_workflow("greeter").await.unwrap().active_version,
        Some(1)
    );
    let versions = client.list_versions("greeter").await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.active).collect::<Vec<_>>(),
        [true, false]
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_draft_cannot_be_activated(pool: PgPool) {
    let server = TestServer::start(pool).await;
    server
        .client
        .create_draft("greeter", &definition("greeter", "hi"))
        .await
        .unwrap();

    assert_eq!(
        failure(server.client.activate_version("greeter", 1).await),
        (StatusCode::CONFLICT, "version_not_published".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn diff_lists_the_changed_fields_as_a_json_patch(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let client = &server.client;
    client
        .create_draft("greeter", &definition("greeter", "hi"))
        .await
        .unwrap();
    client
        .create_draft("greeter", &definition("greeter", "bye"))
        .await
        .unwrap();

    let diff = client.diff_versions("greeter", 1, 2).await.unwrap();

    assert_eq!(
        diff.patch,
        [json!({ "op": "replace", "path": "/steps/0/params/greeting", "value": "bye" })]
    );
    assert!(
        client
            .diff_versions("greeter", 1, 1)
            .await
            .unwrap()
            .patch
            .is_empty()
    );
    assert_eq!(
        failure(client.diff_versions("greeter", 1, 9).await),
        (StatusCode::NOT_FOUND, "version_not_found".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_definition_for_another_workflow_or_a_non_json_body_is_rejected(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let client = &server.client;

    assert_eq!(
        failure(
            client
                .create_draft("greeter", &definition("other", "hi"))
                .await
        ),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            "workflow_id_mismatch".into()
        )
    );
    client
        .create_draft("greeter", &definition("greeter", "hi"))
        .await
        .unwrap();
    assert_eq!(
        failure(
            client
                .replace_draft("greeter", 1, &definition("other", "hi"))
                .await
        ),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            "workflow_id_mismatch".into()
        )
    );
    assert_eq!(
        failure(
            client
                .create_draft("greeter", &json!({ "id": "greeter" }))
                .await
        ),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_definition".into()
        )
    );

    let response = reqwest::Client::new()
        .post(server.url("/v1/workflows/greeter/versions"))
        .bearer_auth(&server.admin_token)
        .header("content-type", "application/json")
        .body("steps: []")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "malformed_request");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn unknown_workflows_and_versions_are_not_found(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let client = &server.client;

    assert_eq!(
        failure(client.get_workflow("nope").await),
        (StatusCode::NOT_FOUND, "workflow_not_found".into())
    );
    assert_eq!(
        failure(client.get_version("nope", 1).await),
        (StatusCode::NOT_FOUND, "workflow_not_found".into())
    );
    client
        .create_draft("greeter", &definition("greeter", "hi"))
        .await
        .unwrap();
    assert_eq!(
        failure(client.get_version("greeter", 2).await),
        (StatusCode::NOT_FOUND, "version_not_found".into())
    );
    assert_eq!(
        failure(client.get_version("greeter", 0).await),
        (StatusCode::BAD_REQUEST, "malformed_request".into())
    );
}
