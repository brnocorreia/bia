//! Runs through the API with a Worker running next to the server: triggering, the Timeline,
//! cancelling, resuming and retrying (ADR-0045).

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bia_client::{AttemptStatus, Client, Error, RunQuery, RunStatus, RunSummary, Scope};
use bia_core::action::flow::Flow;
use bia_core::action::testing::FakeAction;
use bia_core::action::{
    Action, ActionCall, ActionDescriptor, ActionError, BoxFuture, CancellationToken, Effect,
    InMemoryRegistry, Output,
};
use bia_core::dsl::ActionName;
use chrono::TimeDelta;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

mod common;

use common::{TestServer, eventually, spawn_worker};

/// Fails fatally until `fixed` is set, then returns `{ "fixed": true }`.
#[derive(Debug)]
struct Flaky {
    descriptor: ActionDescriptor,
    fixed: AtomicBool,
}

impl Flaky {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            descriptor: ActionDescriptor::new(
                ActionName::new("test.flaky").unwrap(),
                "Fails until it is fixed.",
                Effect::Pure,
            ),
            fixed: AtomicBool::new(false),
        })
    }
}

impl Action for Flaky {
    fn descriptor(&self) -> &ActionDescriptor {
        &self.descriptor
    }

    fn execute(
        &self,
        _call: ActionCall,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        Box::pin(async move {
            if self.fixed.load(Ordering::SeqCst) {
                Ok(json!({ "fixed": true }))
            } else {
                Err(ActionError::fatal("not fixed yet"))
            }
        })
    }
}

struct Fixture {
    server: TestServer,
    _workers: Option<common::Workers>,
    flaky: Arc<Flaky>,
    waiting: Arc<FakeAction>,
    agent: Client,
}

/// A server and, unless `workers` is false, a Worker, over `test.echo`, `test.flaky` (fails
/// until fixed), `test.wait` (runs until cancelled) and the flow Actions.
async fn fixture(pool: PgPool, workers: bool) -> Fixture {
    let flaky = Flaky::new();
    let waiting = Arc::new(FakeAction::new("test.wait").until_cancelled());
    let mut registry = InMemoryRegistry::new();
    let actions: Vec<Arc<dyn Action>> = vec![
        Arc::new(FakeAction::new("test.echo")),
        flaky.clone(),
        waiting.clone(),
    ];
    let flow = Flow::all()
        .into_iter()
        .map(|action| Arc::new(action) as Arc<dyn Action>);
    for action in actions.into_iter().chain(flow) {
        registry.register(action).unwrap();
    }
    let registry = Arc::new(registry);
    let server = TestServer::start_with_registry(pool.clone(), Arc::clone(&registry)).await;
    let _workers = workers.then(|| spawn_worker(&pool, &server.clock, registry));
    let agent = server.client_with(&server.token(Scope::Agent).await);
    Fixture {
        server,
        _workers,
        flaky,
        waiting,
        agent,
    }
}

fn definition(id: &str, steps: Value) -> Value {
    json!({
        "schema_version": 1,
        "id": id,
        "triggers": [{ "id": "by_hand", "type": "manual" }],
        "steps": steps,
    })
}

fn echo(id: &str) -> Value {
    json!({ "id": id, "action": "test.echo", "params": { "step": id } })
}

fn step(id: &str, action: &str) -> Value {
    json!({ "id": id, "action": action, "params": {} })
}

impl Fixture {
    /// Publishes and activates `definition` as the next Version.
    async fn deploy(&self, definition: &Value) -> i32 {
        let id = definition["id"].as_str().unwrap();
        let draft = self.agent.create_draft(id, definition).await.unwrap();
        let number = draft.version.summary.number;
        let admin = &self.server.client;
        admin.publish_version(id, number).await.unwrap();
        admin.activate_version(id, number).await.unwrap();
        number
    }

    async fn status_of(&self, run: Uuid) -> RunStatus {
        self.agent.get_run(run).await.unwrap().summary.status
    }

    async fn wait_for(&self, run: Uuid, status: RunStatus) {
        eventually(&format!("run {run} to be {}", status.as_str()), || async {
            self.status_of(run).await == status
        })
        .await;
    }
}

fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> (StatusCode, String) {
    match result {
        Err(Error::Api { status, body }) => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    }
}

fn steps_of(run: &bia_client::Run) -> Vec<(String, i32, AttemptStatus)> {
    run.timeline
        .iter()
        .map(|e| (e.step_id.clone().unwrap(), e.attempt.unwrap(), e.status))
        .collect()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_triggered_run_completes_and_shows_in_list_and_get_with_its_timeline(pool: PgPool) {
    let f = fixture(pool, true).await;
    f.deploy(&definition(
        "greeter",
        json!([echo("first"), echo("second")]),
    ))
    .await;

    let started = f
        .agent
        .trigger_run("greeter", &json!({ "who": "ana" }), None)
        .await
        .unwrap();
    assert_eq!(started.status, RunStatus::Pending);
    assert_eq!(started.version, 1);
    assert_eq!(
        started.trigger,
        json!({ "type": "manual", "id": "by_hand" })
    );
    f.wait_for(started.id, RunStatus::Succeeded).await;

    let run = f.agent.get_run(started.id).await.unwrap();
    assert_eq!(run.input, json!({ "who": "ana" }));
    assert!(run.summary.finished_at.is_some());
    assert_eq!(
        steps_of(&run),
        [
            ("first".into(), 1, AttemptStatus::Succeeded),
            ("second".into(), 1, AttemptStatus::Succeeded)
        ]
    );
    let first = &run.timeline[0];
    assert_eq!(first.seq, 1);
    assert_eq!(first.action.as_deref(), Some("test.echo"));
    assert_eq!(first.input, Some(json!({ "step": "first" })));
    assert_eq!(first.output, Some(json!({ "step": "first" })));
    assert_eq!(
        first.duration_ms,
        (first.finished_at - first.started_at).num_milliseconds()
    );

    let page = f.agent.list_runs(&RunQuery::default()).await.unwrap();
    assert_eq!(page.runs.len(), 1);
    assert_eq!(page.runs[0].id, started.id);
    assert_eq!(page.next_after, None);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_failing_step_records_its_error_and_appears_in_the_failures_view(pool: PgPool) {
    let f = fixture(pool, true).await;
    f.deploy(&definition(
        "fragile",
        json!([echo("before"), step("boom", "test.flaky")]),
    ))
    .await;

    let started = f
        .agent
        .trigger_run("fragile", &json!({}), None)
        .await
        .unwrap();
    f.wait_for(started.id, RunStatus::Failed).await;

    let run = f.agent.get_run(started.id).await.unwrap();
    assert_eq!(
        steps_of(&run),
        [
            ("before".into(), 1, AttemptStatus::Succeeded),
            ("boom".into(), 1, AttemptStatus::Failed)
        ]
    );
    let boom = &run.timeline[1];
    assert_eq!(boom.error.as_ref().unwrap()["class"], "fatal");
    assert_eq!(boom.error.as_ref().unwrap()["message"], "not fixed yet");
    assert!(boom.duration_ms >= 0);

    let failures = f.agent.list_failures(&RunQuery::default()).await.unwrap();
    assert_eq!(failures.runs.len(), 1);
    assert_eq!(failures.runs[0].summary.id, started.id);
    assert_eq!(failures.runs[0].failure.as_ref().unwrap().seq, 2);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn cancel_moves_a_running_run_to_cancelled(pool: PgPool) {
    let f = fixture(pool, true).await;
    f.deploy(&definition("slow", json!([step("wait", "test.wait")])))
        .await;
    let started = f.agent.trigger_run("slow", &json!({}), None).await.unwrap();
    eventually("the Action to start", || async {
        !f.waiting.calls().is_empty()
    })
    .await;

    let requested = f.agent.cancel_run(started.id).await.unwrap();
    assert!(requested.cancel_requested_at.is_some());
    f.wait_for(started.id, RunStatus::Cancelled).await;

    assert!(f.waiting.saw_cancellation());
    let run = f.agent.get_run(started.id).await.unwrap();
    assert_eq!(
        run.timeline.last().unwrap().status,
        AttemptStatus::Cancelled
    );
    assert_eq!(
        failure(f.agent.cancel_run(started.id).await),
        (StatusCode::CONFLICT, "run_finished".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn resume_reruns_the_failed_step_on_the_original_version_only(pool: PgPool) {
    let f = fixture(pool, true).await;
    let steps = json!([echo("before"), step("boom", "test.flaky"), echo("after")]);
    f.deploy(&definition("fragile", steps)).await;
    let started = f
        .agent
        .trigger_run("fragile", &json!({}), None)
        .await
        .unwrap();
    f.wait_for(started.id, RunStatus::Failed).await;

    // Another Version is active now; the Run still resumes on Version 1.
    f.deploy(&definition("fragile", json!([echo("only")])))
        .await;
    f.flaky.fixed.store(true, Ordering::SeqCst);

    let resumed = f.agent.resume_run(started.id).await.unwrap();
    assert_eq!(resumed.id, started.id);
    assert_eq!(resumed.version, 1);
    f.wait_for(started.id, RunStatus::Succeeded).await;

    let run = f.agent.get_run(started.id).await.unwrap();
    assert_eq!(run.summary.version, 1);
    assert_eq!(
        steps_of(&run),
        [
            ("before".into(), 1, AttemptStatus::Succeeded),
            ("boom".into(), 1, AttemptStatus::Failed),
            ("boom".into(), 2, AttemptStatus::Succeeded),
            ("after".into(), 1, AttemptStatus::Succeeded),
        ]
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn resume_refuses_runs_that_are_not_failed(pool: PgPool) {
    let f = fixture(pool, true).await;
    f.deploy(&definition("fine", json!([echo("only")]))).await;

    let fine = f.agent.trigger_run("fine", &json!({}), None).await.unwrap();
    f.wait_for(fine.id, RunStatus::Succeeded).await;
    assert_eq!(
        failure(f.agent.resume_run(fine.id).await),
        (StatusCode::CONFLICT, "run_not_resumable".into())
    );
    assert_eq!(
        failure(f.agent.resume_run(Uuid::from_u128(1)).await),
        (StatusCode::NOT_FOUND, "run_not_found".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn retry_starts_a_new_run_linked_to_the_original_on_the_same_version(pool: PgPool) {
    let f = fixture(pool, true).await;
    f.deploy(&definition("fragile", json!([step("boom", "test.flaky")])))
        .await;
    let original = f
        .agent
        .trigger_run("fragile", &json!({ "n": 1 }), None)
        .await
        .unwrap();
    f.wait_for(original.id, RunStatus::Failed).await;
    f.deploy(&definition("fragile", json!([echo("only")])))
        .await;
    f.flaky.fixed.store(true, Ordering::SeqCst);

    let retried: RunSummary = f.agent.retry_run(original.id).await.unwrap();
    assert_ne!(retried.id, original.id);
    assert_eq!(retried.retry_of, Some(original.id));
    assert_eq!(retried.version, 1);
    assert_eq!(retried.trigger["type"], "manual");
    f.wait_for(retried.id, RunStatus::Succeeded).await;

    let run = f.agent.get_run(retried.id).await.unwrap();
    assert_eq!(run.input, json!({ "n": 1 }));
    assert_eq!(f.status_of(original.id).await, RunStatus::Failed);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn trigger_is_refused_without_a_workflow_active_version_manual_trigger_or_valid_input(
    pool: PgPool,
) {
    let f = fixture(pool, false).await;
    assert_eq!(
        failure(f.agent.trigger_run("nope", &json!({}), None).await),
        (StatusCode::NOT_FOUND, "workflow_not_found".into())
    );

    let draft = definition("draft-only", json!([echo("only")]));
    f.agent.create_draft("draft-only", &draft).await.unwrap();
    assert_eq!(
        failure(f.agent.trigger_run("draft-only", &json!({}), None).await),
        (StatusCode::CONFLICT, "no_active_version".into())
    );

    let mut hookless = definition("hookless", json!([echo("only")]));
    hookless["triggers"] = json!([]);
    f.deploy(&hookless).await;
    assert_eq!(
        failure(f.agent.trigger_run("hookless", &json!({}), None).await),
        (StatusCode::CONFLICT, "no_manual_trigger".into())
    );

    let mut typed = definition("typed", json!([echo("only")]));
    typed["input_schema"] = json!({
        "type": "object",
        "required": ["email"],
        "properties": { "email": { "type": "string" } },
    });
    f.deploy(&typed).await;
    let (status, code) = failure(
        f.agent
            .trigger_run("typed", &json!({ "email": 1 }), None)
            .await,
    );
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_run_input")
    );
    assert_eq!(
        failure(
            f.agent
                .trigger_run("typed", &json!({}), Some("other"))
                .await
        ),
        (StatusCode::CONFLICT, "no_manual_trigger".into())
    );
    // No Run was created by any refusal.
    assert!(
        f.agent
            .list_runs(&RunQuery::default())
            .await
            .unwrap()
            .runs
            .is_empty()
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn retry_of_an_unfinished_run_is_refused(pool: PgPool) {
    let f = fixture(pool, false).await;
    f.deploy(&definition("greeter", json!([echo("only")])))
        .await;
    let pending = f
        .agent
        .trigger_run("greeter", &json!({}), None)
        .await
        .unwrap();

    assert_eq!(
        failure(f.agent.retry_run(pending.id).await),
        (StatusCode::CONFLICT, "run_not_finished".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn runs_are_filtered_by_workflow_status_and_time_and_paged_newest_first(pool: PgPool) {
    let f = fixture(pool, false).await;
    f.deploy(&definition("a", json!([echo("only")]))).await;
    f.deploy(&definition("b", json!([echo("only")]))).await;

    let mut ids = Vec::new();
    for workflow in ["a", "b", "a", "b", "a"] {
        ids.push(
            f.agent
                .trigger_run(workflow, &json!({}), None)
                .await
                .unwrap()
                .id,
        );
        f.server.clock.advance(TimeDelta::minutes(1));
    }

    // Newest first, two per page, with a cursor until the last page.
    let first = f
        .agent
        .list_runs(&RunQuery {
            limit: Some(2),
            ..RunQuery::default()
        })
        .await
        .unwrap();
    let got: Vec<_> = first.runs.iter().map(|r| r.id).collect();
    assert_eq!(got, [ids[4], ids[3]]);
    let second = f
        .agent
        .list_runs(&RunQuery {
            limit: Some(2),
            after: first.next_after,
            ..RunQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(
        second.runs.iter().map(|r| r.id).collect::<Vec<_>>(),
        [ids[2], ids[1]]
    );
    let last = f
        .agent
        .list_runs(&RunQuery {
            limit: Some(2),
            after: second.next_after,
            ..RunQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(last.runs.iter().map(|r| r.id).collect::<Vec<_>>(), [ids[0]]);
    assert_eq!(last.next_after, None);

    let only_a = RunQuery {
        workflow_id: Some("a".into()),
        ..RunQuery::default()
    };
    let of_a = f.agent.list_runs(&only_a).await.unwrap();
    assert_eq!(
        of_a.runs.iter().map(|r| r.id).collect::<Vec<_>>(),
        [ids[4], ids[2], ids[0]]
    );

    let pending = RunQuery {
        status: Some(RunStatus::Pending),
        ..RunQuery::default()
    };
    assert_eq!(f.agent.list_runs(&pending).await.unwrap().runs.len(), 5);
    let failed = RunQuery {
        status: Some(RunStatus::Failed),
        ..RunQuery::default()
    };
    assert!(f.agent.list_runs(&failed).await.unwrap().runs.is_empty());

    // Runs 1 and 2 were created in [t0 + 1m, t0 + 3m).
    let window = RunQuery {
        created_from: Some(common::t0() + TimeDelta::minutes(1)),
        created_to: Some(common::t0() + TimeDelta::minutes(3)),
        ..RunQuery::default()
    };
    let inside = f.agent.list_runs(&window).await.unwrap();
    assert_eq!(
        inside.runs.iter().map(|r| r.id).collect::<Vec<_>>(),
        [ids[2], ids[1]]
    );

    for bad in [0, 201] {
        let query = RunQuery {
            limit: Some(bad),
            ..RunQuery::default()
        };
        assert_eq!(
            failure(f.agent.list_runs(&query).await),
            (StatusCode::BAD_REQUEST, "malformed_request".into())
        );
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_webhook_token_cannot_touch_runs_and_a_missing_run_is_not_found(pool: PgPool) {
    let f = fixture(pool, false).await;
    let hook = f
        .server
        .client_with(&f.server.webhook_token("greeter", "hook").await);

    assert_eq!(
        failure(hook.list_runs(&RunQuery::default()).await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );
    assert_eq!(
        failure(f.agent.get_run(Uuid::from_u128(1)).await),
        (StatusCode::NOT_FOUND, "run_not_found".into())
    );
}
