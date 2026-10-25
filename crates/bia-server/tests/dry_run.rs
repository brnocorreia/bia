//! Dry Run through the API: pure Actions run for real, side effects return Mocks, waits
//! resolve instantly, and nothing is stored (ADR-0046).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use bia_client::{AttemptStatus, Client, Error, RunStatus, Scope};
use bia_core::action::flow::Flow;
use bia_core::action::testing::FakeAction;
use bia_core::action::{
    Action, ActionCall, ActionDescriptor, ActionError, ActionExample, BoxFuture, CancellationToken,
    Effect, InMemoryRegistry, Output,
};
use bia_core::dsl::ActionName;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use wiremock::MockServer;

mod common;

use common::{TestServer, eventually, spawn_worker};

struct Fixture {
    server: TestServer,
    agent: Client,
    registry: Arc<bia_core::action::InMemoryRegistry>,
    pool: PgPool,
}

/// A side-effecting Action that really sends a GET to `params.url`, with an example.
#[derive(Debug)]
struct Http {
    descriptor: ActionDescriptor,
    client: reqwest::Client,
}

impl Http {
    fn new() -> Self {
        let descriptor = ActionDescriptor::new(
            ActionName::new("test.http").unwrap(),
            "Sends a GET request.",
            Effect::SideEffecting,
        )
        .example(ActionExample::new(
            "Check a site",
            json!({ "url": "https://example.com/health" })
                .as_object()
                .unwrap()
                .clone(),
            json!({ "status": 200, "body": { "from": "example" } }),
        ));
        Self {
            descriptor,
            client: reqwest::Client::new(),
        }
    }
}

impl Action for Http {
    fn descriptor(&self) -> &ActionDescriptor {
        &self.descriptor
    }

    fn execute(
        &self,
        call: ActionCall,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        Box::pin(async move {
            let url = call.params["url"].as_str().unwrap_or_default();
            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| ActionError::retryable(e.to_string()))?;
            Ok(json!({ "status": response.status().as_u16() }))
        })
    }
}

/// A pure Action returning how many `params.items` there are.
#[derive(Debug)]
struct Count(ActionDescriptor);

impl Action for Count {
    fn descriptor(&self) -> &ActionDescriptor {
        &self.0
    }

    fn execute(
        &self,
        call: ActionCall,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        Box::pin(async move { Ok(json!(call.params["items"].as_array().map_or(0, Vec::len))) })
    }
}

/// `test.http`, `test.count`, the flow Actions, `test.fail`, which always fails fatally, and
/// `test.bare`, which has side effects and no example.
async fn fixture(pool: PgPool) -> Fixture {
    let mut registry = InMemoryRegistry::new();
    let count = Count(ActionDescriptor::new(
        ActionName::new("test.count").unwrap(),
        "Counts items.",
        Effect::Pure,
    ));
    let flow = Flow::all()
        .into_iter()
        .map(|action| Arc::new(action) as Arc<dyn Action>);
    for action in flow.chain([Arc::new(Http::new()) as Arc<dyn Action>, Arc::new(count)]) {
        registry.register(action).unwrap();
    }
    let fail = FakeAction::new("test.fail").failing(ActionError::fatal("it always fails"));
    registry
        .register(Arc::new(fail) as Arc<dyn Action>)
        .unwrap();
    let bare = FakeAction::new("test.bare").with_descriptor(|d| d.effect = Effect::SideEffecting);
    registry
        .register(Arc::new(bare) as Arc<dyn Action>)
        .unwrap();
    let registry = Arc::new(registry);
    let server = TestServer::start_with_registry(pool.clone(), Arc::clone(&registry)).await;
    let agent = server.client_with(&server.token(Scope::Agent).await);
    Fixture {
        server,
        agent,
        registry,
        pool,
    }
}

fn definition(steps: Value) -> Value {
    json!({
        "schema_version": 1,
        "id": "uptime",
        "triggers": [{ "id": "by_hand", "type": "manual" }],
        "steps": steps,
    })
}

fn http_step(id: &str, url: &str) -> Value {
    json!({ "id": id, "action": "test.http", "params": { "url": url } })
}

fn count_step() -> Value {
    json!({
        "id": "count",
        "action": "test.count",
        "params": { "items": "{{ steps.fetch.output.body.items }}" },
    })
}

fn calc_step() -> Value {
    json!({ "id": "calc", "action": "test.count", "params": { "items": [1, 2] } })
}

/// How many rows a Dry Run must have left in `workflows`, `runs`, `timeline_entries` and `jobs`.
async fn stored_rows(pool: &PgPool) -> [i64; 3] {
    sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT (SELECT count(*) FROM runs), (SELECT count(*) FROM timeline_entries), \
         (SELECT count(*) FROM jobs)",
    )
    .fetch_one(pool)
    .await
    .map(|(runs, entries, jobs)| [runs, entries, jobs])
    .unwrap()
}

fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> (StatusCode, String) {
    match result {
        Err(Error::Api { status, body }) => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_dry_run_mocks_the_http_call_runs_the_transform_for_real_and_stores_nothing(
    pool: PgPool,
) {
    let f = fixture(pool).await;
    let outbound = MockServer::start().await;
    let draft = definition(json!([http_step("fetch", &outbound.uri()), count_step()]));
    f.agent.create_draft("uptime", &draft).await.unwrap();

    let mocks = json!({ "fetch": { "output": { "status": 200, "body": { "items": [1, 2, 3] } } } });
    let report = f
        .agent
        .dry_run_version("uptime", 1, &json!({}), &mocks)
        .await
        .unwrap();

    assert!(report.dry_run);
    assert_eq!(report.status, RunStatus::Succeeded);
    let steps: Vec<_> = report
        .timeline
        .iter()
        .map(|e| {
            (
                e.seq,
                e.step_id.as_deref().unwrap(),
                e.action.as_deref().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        steps,
        [(1, "fetch", "test.http"), (2, "count", "test.count")]
    );
    assert_eq!(
        report.timeline[0].output,
        Some(json!({ "status": 200, "body": { "items": [1, 2, 3] } }))
    );
    assert_eq!(report.timeline[1].output, Some(json!(3)));

    assert!(outbound.received_requests().await.unwrap().is_empty());
    assert_eq!(
        stored_rows(&f.pool).await,
        [0, 0, 0],
        "runs, entries and jobs"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_dry_run_of_a_definition_that_is_not_stored_works_too(pool: PgPool) {
    let f = fixture(pool).await;
    let draft = definition(json!([http_step("fetch", "https://example.com")]));
    let report = f
        .agent
        .dry_run_definition(&draft, &json!({}), &json!({}))
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Succeeded);
    assert_eq!(report.timeline.len(), 1);
    assert_eq!(stored_rows(&f.pool).await, [0, 0, 0]);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn without_a_mock_the_actions_first_example_output_is_used(pool: PgPool) {
    let f = fixture(pool).await;
    let draft = definition(json!([http_step("fetch", "https://example.com/health")]));
    f.agent.create_draft("uptime", &draft).await.unwrap();

    let report = f
        .agent
        .dry_run_version("uptime", 1, &json!({}), &json!({}))
        .await
        .unwrap();

    assert_eq!(report.status, RunStatus::Succeeded);
    assert_eq!(
        report.timeline[0].output.as_ref().unwrap()["body"]["from"],
        "example"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_side_effecting_action_without_a_mock_or_example_fails_with_no_mock(pool: PgPool) {
    let f = fixture(pool).await;
    let draft = definition(json!([{ "id": "go", "action": "test.bare", "params": {} }]));
    let report = f
        .agent
        .dry_run_definition(&draft, &json!({}), &json!({}))
        .await
        .unwrap();

    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(
        report.timeline[0].error.as_ref().unwrap()["code"],
        "no_mock"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_mocked_error_drives_the_retry_policy_then_the_failure_path(pool: PgPool) {
    let f = fixture(pool).await;
    let retrying = |max_attempts: u32| {
        let mut step = http_step("fetch", "https://example.com");
        step["retry"] =
            json!({ "max_attempts": max_attempts, "backoff": "fixed", "initial": "1s" });
        definition(json!([step]))
    };
    let flaky = json!({ "class": "retryable", "message": "503 from upstream" });

    // Two failures, then success.
    let mocks = json!({ "fetch": { "attempts": [
        { "error": flaky }, { "error": flaky }, { "output": { "status": 200 } }
    ] } });
    let report = f
        .agent
        .dry_run_definition(&retrying(3), &json!({}), &mocks)
        .await
        .unwrap();
    let outcome: Vec<_> = report
        .timeline
        .iter()
        .map(|e| (e.attempt.unwrap(), e.status))
        .collect();
    assert_eq!(
        outcome,
        [
            (1, AttemptStatus::Failed),
            (2, AttemptStatus::Failed),
            (3, AttemptStatus::Succeeded)
        ]
    );
    assert_eq!(report.status, RunStatus::Succeeded);
    assert_eq!(
        report.timeline[0].error.as_ref().unwrap()["class"],
        "retryable"
    );

    // The same error on every Attempt exhausts the policy.
    let always = json!({ "fetch": { "error": flaky } });
    let report = f
        .agent
        .dry_run_definition(&retrying(3), &json!({}), &always)
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.timeline.len(), 3);

    // A fatal error is not retried.
    let fatal = json!({ "fetch": { "error": { "class": "fatal", "message": "404" } } });
    let report = f
        .agent
        .dry_run_definition(&retrying(3), &json!({}), &fatal)
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.timeline.len(), 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_three_day_sleep_completes_instantly_and_shows_its_time(pool: PgPool) {
    let f = fixture(pool).await;
    let draft = definition(json!([
        { "id": "nap", "action": "flow.sleep", "params": { "duration": "3d" } },
        { "id": "after", "action": "test.count", "params": { "items": [1, 2] } },
    ]));

    let started = std::time::Instant::now();
    let report = f
        .agent
        .dry_run_definition(&draft, &json!({}), &json!({}))
        .await
        .unwrap();

    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(report.status, RunStatus::Succeeded);
    assert_eq!(report.timeline.len(), 2);
    let nap = &report.timeline[0];
    assert_eq!(nap.action.as_deref(), Some("flow.sleep"));
    assert_eq!(nap.duration_ms, 3 * 24 * 3600 * 1000);
    assert_eq!(report.timeline[1].output, Some(json!(2)));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn branches_and_conditions_follow_the_input_as_in_a_real_run(pool: PgPool) {
    let f = fixture(pool).await;
    let draft = definition(json!([
        {
            "id": "check",
            "action": "flow.branch",
            "when": "input.up",
            "then": [{ "id": "ok", "action": "test.count", "params": { "items": [] } }],
            "else": [{ "id": "alert", "action": "test.count", "params": { "items": [] } }],
        },
        {
            "id": "skipped",
            "action": "test.count",
            "if": "false",
            "params": { "items": [] },
        },
    ]));

    let report = f
        .agent
        .dry_run_definition(&draft, &json!({ "up": false }), &json!({}))
        .await
        .unwrap();

    let ids: Vec<_> = report
        .timeline
        .iter()
        .map(|e| (e.step_id.clone().unwrap(), e.status))
        .collect();
    assert_eq!(
        ids,
        [
            ("check".to_owned(), AttemptStatus::Succeeded),
            ("alert".to_owned(), AttemptStatus::Succeeded),
            ("skipped".to_owned(), AttemptStatus::Skipped),
        ]
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_secret_reference_needs_no_secret_and_is_shown_as_written(pool: PgPool) {
    let f = fixture(pool).await;
    let draft = definition(json!([{
        "id": "fetch",
        "action": "test.http",
        "params": { "url": "https://example.com", "key": "{{ secrets.API_KEY }}" },
    }]));

    let report = f
        .agent
        .dry_run_definition(&draft, &json!({}), &json!({}))
        .await
        .unwrap();

    assert_eq!(report.status, RunStatus::Succeeded);
    let input = report.timeline[0].input.as_ref().unwrap();
    assert_eq!(input["key"], "{{ secrets.API_KEY }}");
    assert!(
        !serde_json::to_string(&report.timeline)
            .unwrap()
            .contains("<secret")
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_pure_action_can_be_mocked_to_force_a_failure(pool: PgPool) {
    let f = fixture(pool).await;
    let draft = definition(json!([calc_step()]));
    let mocks = json!({ "calc": { "error": { "class": "fatal", "message": "forced" } } });
    let report = f
        .agent
        .dry_run_definition(&draft, &json!({}), &mocks)
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(
        report.timeline[0].error.as_ref().unwrap()["message"],
        "forced"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn input_that_breaks_the_input_schema_fails_the_dry_run_like_a_real_run(pool: PgPool) {
    let f = fixture(pool).await;
    let mut draft = definition(json!([calc_step()]));
    draft["input_schema"] = json!({
        "type": "object",
        "required": ["url"],
        "properties": { "url": { "type": "string" } },
    });
    let report = f
        .agent
        .dry_run_definition(&draft, &json!({}), &json!({}))
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Failed);
    assert_eq!(report.timeline.len(), 1);
    assert_eq!(report.timeline[0].step_id, None);
    assert_eq!(
        report.timeline[0].error.as_ref().unwrap()["code"],
        "invalid_input"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn bad_mocks_and_bad_definitions_are_refused_before_anything_runs(pool: PgPool) {
    let f = fixture(pool).await;
    let draft = definition(json!([
        http_step("fetch", "https://example.com"),
        { "id": "nap", "action": "flow.sleep", "params": { "duration": "1s" } },
    ]));
    let dry = |mocks: Value| {
        let (agent, draft) = (f.agent.clone(), draft.clone());
        async move { agent.dry_run_definition(&draft, &json!({}), &mocks).await }
    };

    let typo = failure(dry(json!({ "fetsh": { "output": 1 } })).await);
    assert_eq!(
        typo,
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_mock".into())
    );
    let flow = failure(dry(json!({ "nap": { "output": 1 } })).await);
    assert_eq!(
        flow,
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_mock".into())
    );
    for mock in [
        json!({ "fetch": {} }),
        json!({ "fetch": { "output": 1, "error": { "class": "fatal", "message": "x" } } }),
        json!({ "fetch": { "error": { "class": "sad", "message": "x" } } }),
        json!({ "fetch": { "attempts": [] } }),
        json!({ "fetch": { "outptu": 1 } }),
        json!({ "fetch": 7 }),
    ] {
        let (status, code) = failure(dry(json!(mock)).await);
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, "invalid_mock")
        );
    }

    let broken = definition(json!([{ "id": "x", "action": "no.such", "params": {} }]));
    let refused = f
        .agent
        .dry_run_definition(&broken, &json!({}), &json!({}))
        .await;
    assert_eq!(failure(refused).1, "invalid_definition");

    let missing = f
        .agent
        .dry_run_version("ghost", 1, &json!({}), &json!({}))
        .await;
    assert_eq!(
        failure(missing),
        (StatusCode::NOT_FOUND, "workflow_not_found".into())
    );
}

fn kinds(entry: &bia_client::TimelineEntry) -> BTreeMap<String, &'static str> {
    let Value::Object(fields) = serde_json::to_value(entry).unwrap() else {
        panic!("a Timeline entry is an object");
    };
    fields
        .into_iter()
        .map(|(key, value)| {
            let kind = match value {
                Value::Null => "null",
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            (key, kind)
        })
        .collect()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_dry_run_timeline_has_the_schema_of_a_real_runs_timeline(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let _worker = spawn_worker(&pool, &f.server.clock, Arc::clone(&f.registry));
    let steps = json!([
        { "id": "calc", "action": "test.count", "params": { "items": [1, 2] } },
        { "id": "skipped", "action": "test.count", "if": "false", "params": { "items": [] } },
        { "id": "boom", "action": "test.fail", "params": {} },
    ]);
    let workflow = definition(steps);
    f.agent.create_draft("uptime", &workflow).await.unwrap();
    f.server.client.publish_version("uptime", 1).await.unwrap();
    f.server.client.activate_version("uptime", 1).await.unwrap();

    let started = f
        .agent
        .trigger_run("uptime", &json!({}), None)
        .await
        .unwrap();
    eventually("the real run to fail", || async {
        f.agent.get_run(started.id).await.unwrap().summary.status == RunStatus::Failed
    })
    .await;
    let real = f.agent.get_run(started.id).await.unwrap().timeline;

    let mocks = json!({ "boom": { "error": { "class": "fatal", "message": "it always fails" } } });
    let dry = f
        .agent
        .dry_run_version("uptime", 1, &json!({}), &mocks)
        .await
        .unwrap()
        .timeline;

    assert_eq!(real.len(), 3);
    assert_eq!(dry.len(), real.len());
    for (dry, real) in dry.iter().zip(&real) {
        assert_eq!(
            kinds(dry),
            kinds(real),
            "entry {} differs in shape",
            real.seq
        );
        assert_eq!((dry.seq, dry.status), (real.seq, real.status));
    }
}
