//! Approval Requests through the API: an `agent` asks, the admin decides, and approving
//! performs the change (ADR-0008, ADR-0066).

#![allow(clippy::unwrap_used)]

use bia_client::{
    ApprovalAction, ApprovalStatus, Client, Error, NewApprovalRequest, QueueSettings, Scope,
    VersionState,
};
use chrono::TimeDelta;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

mod common;

use common::TestServer;

fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> (StatusCode, String) {
    match result {
        Err(Error::Api { status, body }) => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    }
}

fn definition(id: &str, queue: &str) -> Value {
    json!({
        "schema_version": 1,
        "id": id,
        "triggers": [{ "id": "by_hand", "type": "manual" }],
        "steps": [{
            "id": "send",
            "action": "queue.publish",
            "params": { "queue": queue, "body": {} },
        }],
    })
}

fn request(workflow: &str, number: i32, action: ApprovalAction) -> NewApprovalRequest {
    NewApprovalRequest {
        workflow_id: workflow.to_owned(),
        number,
        action,
        note: Some("ready".to_owned()),
    }
}

struct Fixture {
    server: TestServer,
    agent: Client,
}

async fn fixture(pool: PgPool) -> Fixture {
    let server = TestServer::start(pool).await;
    server
        .client
        .put_queue("emails", &QueueSettings::default())
        .await
        .unwrap();
    let agent = server.client_with(&server.token(Scope::Agent).await);
    Fixture { server, agent }
}

impl Fixture {
    /// A Draft of `greeter` made by the agent, which is as far as an agent can go.
    async fn draft(&self, queue: &str) -> i32 {
        self.agent
            .create_draft("greeter", &definition("greeter", queue))
            .await
            .unwrap()
            .version
            .summary
            .number
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_agent_creates_reads_and_withdraws_requests_but_cannot_decide_them(pool: PgPool) {
    let f = fixture(pool).await;
    let number = f.draft("emails").await;

    let created = f
        .agent
        .request_approval(&request("greeter", number, ApprovalAction::Publish))
        .await
        .unwrap();
    assert_eq!(created.status, ApprovalStatus::Pending);
    assert_eq!(created.requester_name, "test-agent");
    assert_eq!(created.note.as_deref(), Some("ready"));
    assert_eq!(created.created_at, common::t0());
    assert!(created.content_hash.is_some() && created.closed_at.is_none());

    assert_eq!(
        f.agent.get_approval_request(created.id).await.unwrap(),
        created
    );
    let listed = f.agent.list_approval_requests(None, None).await.unwrap();
    assert_eq!(listed, std::slice::from_ref(&created));

    for decision in [
        f.agent.approve_approval_request(created.id, None).await,
        f.agent.reject_approval_request(created.id, None).await,
    ] {
        assert_eq!(
            failure(decision),
            (StatusCode::FORBIDDEN, "insufficient_scope".into())
        );
    }

    f.server.clock.advance(TimeDelta::minutes(5));
    let withdrawn = f.agent.withdraw_approval_request(created.id).await.unwrap();
    assert_eq!(withdrawn.status, ApprovalStatus::Withdrawn);
    assert_eq!(
        withdrawn.closed_at,
        Some(common::t0() + TimeDelta::minutes(5))
    );
    assert_eq!(
        failure(f.agent.withdraw_approval_request(created.id).await),
        (StatusCode::CONFLICT, "approval_request_not_pending".into())
    );
    assert_eq!(
        failure(f.agent.get_approval_request(Uuid::new_v4()).await),
        (StatusCode::NOT_FOUND, "approval_request_not_found".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn approving_publish_and_activate_publishes_and_activates_as_the_decider(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let number = f.draft("emails").await;
    let created = f
        .agent
        .request_approval(&request(
            "greeter",
            number,
            ApprovalAction::PublishAndActivate,
        ))
        .await
        .unwrap();

    f.server.clock.advance(TimeDelta::minutes(1));
    let admin = &f.server.client;
    let approved = admin
        .approve_approval_request(created.id, Some("looks good"))
        .await
        .unwrap();

    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(approved.comment.as_deref(), Some("looks good"));
    let user: Uuid = sqlx::query_scalar("SELECT id FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(approved.decided_by, Some(user));
    let version = admin.get_version("greeter", number).await.unwrap();
    assert_eq!(version.summary.state, VersionState::Published);
    assert!(version.summary.active);
    assert_eq!(
        version.summary.published_at,
        Some(common::t0() + TimeDelta::minutes(1))
    );
    assert_eq!(
        failure(admin.approve_approval_request(created.id, None).await),
        (StatusCode::CONFLICT, "approval_request_not_pending".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_failing_publish_leaves_the_version_and_the_request_unchanged(pool: PgPool) {
    let f = fixture(pool).await;
    let number = f.draft("emails").await;
    let created = f
        .agent
        .request_approval(&request(
            "greeter",
            number,
            ApprovalAction::PublishAndActivate,
        ))
        .await
        .unwrap();
    // The Queue goes away after the request, so publishing now fails its checks.
    f.server.client.delete_queue("emails").await.unwrap();

    let admin = &f.server.client;
    let Err(Error::Api { status, body }) = admin.approve_approval_request(created.id, None).await
    else {
        panic!("approving should fail");
    };
    assert_eq!(
        (status, body.code.as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_definition")
    );
    assert_eq!(body.diagnostics[0].code.as_str(), "unknown_queue");

    let version = admin.get_version("greeter", number).await.unwrap();
    assert_eq!(version.summary.state, VersionState::Draft);
    assert_eq!(
        admin.get_workflow("greeter").await.unwrap().active_version,
        None
    );
    assert_eq!(
        admin.get_approval_request(created.id).await.unwrap().status,
        ApprovalStatus::Pending
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn replacing_the_draft_makes_the_request_stale_and_unapprovable(pool: PgPool) {
    let f = fixture(pool).await;
    let number = f.draft("emails").await;
    let created = f
        .agent
        .request_approval(&request("greeter", number, ApprovalAction::Publish))
        .await
        .unwrap();

    f.server.clock.advance(TimeDelta::minutes(2));
    let mut changed = definition("greeter", "emails");
    changed["steps"][0]["params"]["body"] = json!({ "new": true });
    f.agent
        .replace_draft("greeter", number, &changed)
        .await
        .unwrap();

    let stale = f.agent.get_approval_request(created.id).await.unwrap();
    assert_eq!(stale.status, ApprovalStatus::Stale);
    assert_eq!(stale.closed_at, Some(common::t0() + TimeDelta::minutes(2)));
    let admin = &f.server.client;
    assert_eq!(
        failure(admin.approve_approval_request(created.id, None).await),
        (StatusCode::CONFLICT, "approval_request_not_pending".into())
    );
    assert_eq!(
        failure(admin.reject_approval_request(created.id, None).await),
        (StatusCode::CONFLICT, "approval_request_not_pending".into())
    );
    let version = admin.get_version("greeter", number).await.unwrap();
    assert_eq!(version.summary.state, VersionState::Draft);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_request_for_an_invalid_draft_returns_its_diagnostics_and_creates_nothing(pool: PgPool) {
    let f = fixture(pool).await;
    let number = f.draft("emals").await;

    let Err(Error::Api { status, body }) = f
        .agent
        .request_approval(&request("greeter", number, ApprovalAction::Publish))
        .await
    else {
        panic!("the request should be refused");
    };
    assert_eq!(
        (status, body.code.as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_definition")
    );
    assert_eq!(body.diagnostics[0].code.as_str(), "unknown_queue");
    assert!(
        f.agent
            .list_approval_requests(None, None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_second_request_for_the_workflow_withdraws_the_first(pool: PgPool) {
    let f = fixture(pool).await;
    let first_draft = f.draft("emails").await;
    let first = f
        .agent
        .request_approval(&request("greeter", first_draft, ApprovalAction::Publish))
        .await
        .unwrap();
    let second_draft = f.draft("emails").await;
    f.server.clock.advance(TimeDelta::seconds(30));
    let second = f
        .agent
        .request_approval(&request("greeter", second_draft, ApprovalAction::Publish))
        .await
        .unwrap();

    let first = f.agent.get_approval_request(first.id).await.unwrap();
    assert_eq!(first.status, ApprovalStatus::Withdrawn);
    assert_eq!(second.status, ApprovalStatus::Pending);

    let pending = f
        .agent
        .list_approval_requests(Some(ApprovalStatus::Pending), Some("greeter"))
        .await
        .unwrap();
    assert_eq!(pending, std::slice::from_ref(&second));
    let all = f.agent.list_approval_requests(None, None).await.unwrap();
    assert_eq!(
        all.iter().map(|r| r.id).collect::<Vec<_>>(),
        [second.id, first.id]
    );
    assert!(
        f.agent
            .list_approval_requests(None, Some("other"))
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn activate_requests_need_a_published_version_and_make_rollbacks(pool: PgPool) {
    let f = fixture(pool).await;
    let admin = &f.server.client;
    let first = f.draft("emails").await;
    assert_eq!(
        failure(
            f.agent
                .request_approval(&request("greeter", first, ApprovalAction::Activate))
                .await
        ),
        (StatusCode::CONFLICT, "version_not_published".into())
    );
    admin.publish_version("greeter", first).await.unwrap();
    admin.activate_version("greeter", first).await.unwrap();
    let second = f.draft("emails").await;
    admin.publish_version("greeter", second).await.unwrap();
    admin.activate_version("greeter", second).await.unwrap();

    assert_eq!(
        failure(
            f.agent
                .request_approval(&request("greeter", second, ApprovalAction::Publish))
                .await
        ),
        (StatusCode::CONFLICT, "version_published".into())
    );
    let rollback = f
        .agent
        .request_approval(&request("greeter", first, ApprovalAction::Activate))
        .await
        .unwrap();
    assert_eq!(rollback.content_hash, None);
    admin
        .approve_approval_request(rollback.id, None)
        .await
        .unwrap();
    assert_eq!(
        admin.get_workflow("greeter").await.unwrap().active_version,
        Some(first)
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn rejecting_keeps_the_comment_and_changes_nothing(pool: PgPool) {
    let f = fixture(pool).await;
    let number = f.draft("emails").await;
    let created = f
        .agent
        .request_approval(&request("greeter", number, ApprovalAction::Publish))
        .await
        .unwrap();

    let rejected = f
        .server
        .client
        .reject_approval_request(created.id, Some("not yet"))
        .await
        .unwrap();
    assert_eq!(rejected.status, ApprovalStatus::Rejected);
    assert_eq!(rejected.comment.as_deref(), Some("not yet"));
    assert!(rejected.decided_by.is_some());
    let version = f
        .server
        .client
        .get_version("greeter", number)
        .await
        .unwrap();
    assert_eq!(version.summary.state, VersionState::Draft);
    assert_eq!(
        failure(f.agent.withdraw_approval_request(created.id).await),
        (StatusCode::CONFLICT, "approval_request_not_pending".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn requests_are_refused_for_unknown_workflows_and_versions(pool: PgPool) {
    let f = fixture(pool).await;
    assert_eq!(
        failure(
            f.agent
                .request_approval(&request("ghost", 1, ApprovalAction::Publish))
                .await
        ),
        (StatusCode::NOT_FOUND, "workflow_not_found".into())
    );
    f.draft("emails").await;
    assert_eq!(
        failure(
            f.agent
                .request_approval(&request("greeter", 9, ApprovalAction::Publish))
                .await
        ),
        (StatusCode::NOT_FOUND, "version_not_found".into())
    );
}
