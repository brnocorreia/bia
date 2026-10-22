//! The interpreter drives sequential Runs to `succeeded` or `failed` through Jobs, with
//! retries, timeouts, Idempotency Keys and a complete Timeline (ADR-0001, ADR-0010, ADR-0012).

// `allow-unwrap-in-tests` covers `#[test]` functions only, not the helpers they share.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use bia_core::action::testing::FakeAction;
use bia_core::action::{
    Action, ActionCall, ActionDescriptor, ActionError, BoxFuture, CancellationToken, Effect,
    InMemoryRegistry, Output,
};
use bia_core::clock::{Clock, ManualClock};
use bia_core::dsl::{ActionName, Definition};
use bia_engine::{Advanced, EngineError, Interpreter, SecretResolver, start_run};
use bia_store::StoreError;
use bia_store::job::{Claimed, JobRepository, PgJobRepository};
use bia_store::run::{PgRunRepository, Run, RunId, RunRepository, RunStatus};
use bia_store::timeline::{AttemptStatus, PgTimelineRepository, TimelineEntry, TimelineRepository};
use bia_store::version::{PgVersionRepository, VersionRepository};
use bia_store::workflow::{PgWorkflowRepository, WorkflowRepository};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;

const LEASE: TimeDelta = TimeDelta::seconds(30);

fn t0() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

struct Harness {
    pool: PgPool,
    clock: ManualClock,
    interpreter: Interpreter,
}

impl Harness {
    fn new(pool: PgPool, clock: &ManualClock, actions: Vec<Arc<dyn Action>>) -> Self {
        let mut registry = InMemoryRegistry::new();
        for action in actions {
            registry.register(action).unwrap();
        }
        let clock = clock.clone();
        let interpreter =
            Interpreter::new(pool.clone(), Arc::new(registry), Arc::new(clock.clone()));
        Self {
            pool,
            clock,
            interpreter,
        }
    }

    fn with_secrets(mut self, secrets: impl SecretResolver + 'static) -> Self {
        self.interpreter = self.interpreter.with_secrets(Arc::new(secrets));
        self
    }

    /// Publishes `definition` as Version 1 of its Workflow and starts a Run on it.
    async fn start(&self, definition: Value, input: Value) -> RunId {
        let definition = Definition::from_value(definition).unwrap();
        let mut conn = self.pool.acquire().await.unwrap();
        PgWorkflowRepository
            .create(&mut conn, &definition.id)
            .await
            .unwrap();
        let draft = PgVersionRepository
            .create_draft(&mut conn, &definition)
            .await
            .unwrap();
        PgVersionRepository
            .publish(
                &mut conn,
                &draft.workflow_id,
                draft.number,
                self.clock.now(),
            )
            .await
            .unwrap();
        let run = start_run(
            &mut conn,
            &definition.id,
            draft.number,
            input,
            json!({ "type": "manual" }),
            self.clock.now(),
        )
        .await
        .unwrap();
        run.id
    }

    /// Claims the next due Job as `worker`, if any.
    async fn claim(&self, worker: &str) -> Option<Claimed> {
        let mut conn = self.pool.acquire().await.unwrap();
        PgJobRepository
            .claim(&mut conn, worker, 1, LEASE, self.clock.now())
            .await
            .unwrap()
            .pop()
    }

    /// Advances Runs until no Job is due at the Clock's current time.
    async fn drive(&self) -> Vec<Advanced> {
        let mut advanced = Vec::new();
        while let Some(claimed) = self.claim("worker-a").await {
            advanced.push(self.interpreter.advance(&claimed).await.unwrap());
        }
        advanced
    }

    async fn run(&self, id: RunId) -> Run {
        let mut conn = self.pool.acquire().await.unwrap();
        PgRunRepository.get(&mut conn, id).await.unwrap().unwrap()
    }

    async fn timeline(&self, id: RunId) -> Vec<TimelineEntry> {
        let mut conn = self.pool.acquire().await.unwrap();
        PgTimelineRepository.list(&mut conn, id).await.unwrap()
    }

    async fn jobs_left(&self, id: RunId) -> usize {
        let mut conn = self.pool.acquire().await.unwrap();
        PgJobRepository
            .list_for_run(&mut conn, id)
            .await
            .unwrap()
            .len()
    }
}

fn step_id(entry: &TimelineEntry) -> &str {
    entry.step.as_ref().unwrap().step_id.as_str()
}

fn attempt(entry: &TimelineEntry) -> i32 {
    entry.step.as_ref().unwrap().attempt
}

fn keys(calls: &[ActionCall]) -> Vec<String> {
    calls
        .iter()
        .map(|call| call.idempotency_key.as_str().to_owned())
        .collect()
}

fn descriptor(name: &str) -> ActionDescriptor {
    ActionDescriptor::new(
        ActionName::new(name).unwrap(),
        "A test Action.",
        Effect::SideEffecting,
    )
}

/// Fails with a `retryable` error a set number of times, then succeeds; moves the Clock by
/// `takes` during every call, so Attempts have a duration.
#[derive(Debug)]
struct Flaky {
    descriptor: ActionDescriptor,
    clock: ManualClock,
    takes: TimeDelta,
    failures_left: Mutex<u32>,
    calls: Mutex<Vec<ActionCall>>,
}

impl Flaky {
    fn new(clock: &ManualClock, failures: u32, takes: TimeDelta) -> Self {
        Self {
            descriptor: descriptor("test.flaky"),
            clock: clock.clone(),
            takes,
            failures_left: Mutex::new(failures),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<ActionCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl Action for Flaky {
    fn descriptor(&self) -> &ActionDescriptor {
        &self.descriptor
    }

    fn execute(
        &self,
        call: ActionCall,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(call);
            self.clock.advance(self.takes);
            let mut failures_left = self.failures_left.lock().unwrap();
            if *failures_left > 0 {
                *failures_left -= 1;
                return Err(ActionError::retryable("upstream returned 503"));
            }
            Ok(json!({ "delivered": true }))
        })
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_three_step_run_succeeds_and_each_step_reads_the_previous_output(pool: PgPool) {
    let echo = Arc::new(FakeAction::new("test.echo"));
    let h = Harness::new(pool, &ManualClock::new(t0()), vec![echo.clone()]);
    let run = h
        .start(
            json!({
                "schema_version": 1,
                "id": "sequence",
                "input_schema": { "type": "object", "required": ["name"] },
                "steps": [
                    { "id": "first", "action": "test.echo", "params": { "n": 1 } },
                    {
                        "id": "second",
                        "action": "test.echo",
                        "params": { "n": "{{ steps.first.output.n + 1 }}" },
                    },
                    {
                        "id": "third",
                        "action": "test.echo",
                        "params": {
                            "n": "{{ steps.second.output.n * 10 }}",
                            "greeting": "hello {{ input.name }}",
                        },
                    },
                ],
            }),
            json!({ "name": "ada" }),
        )
        .await;

    let advanced = h.drive().await;

    let statuses: Vec<_> = advanced.iter().map(|advanced| advanced.status).collect();
    assert_eq!(
        statuses,
        [
            RunStatus::Running,
            RunStatus::Running,
            RunStatus::Running,
            RunStatus::Succeeded,
        ]
    );
    let finished = h.run(run).await;
    assert_eq!(finished.status, RunStatus::Succeeded);
    assert_eq!(finished.finished_at, Some(t0()));
    assert_eq!(h.jobs_left(run).await, 0);

    let timeline = h.timeline(run).await;
    let summary: Vec<_> = timeline
        .iter()
        .map(|entry| {
            (
                step_id(entry),
                attempt(entry),
                entry.status,
                entry.output.clone(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "first",
                1,
                AttemptStatus::Succeeded,
                Some(json!({ "n": 1 }))
            ),
            (
                "second",
                1,
                AttemptStatus::Succeeded,
                Some(json!({ "n": 2 }))
            ),
            (
                "third",
                1,
                AttemptStatus::Succeeded,
                Some(json!({ "n": 20, "greeting": "hello ada" }))
            ),
        ]
    );
    assert_eq!(
        timeline[1].input,
        Some(json!({ "n": 2 })),
        "the Timeline records Params with their Templates resolved"
    );
    assert_eq!(echo.calls().len(), 3);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_retryable_failure_retries_after_its_backoff_with_the_same_idempotency_key(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let flaky = Arc::new(Flaky::new(&clock, 2, TimeDelta::seconds(2)));
    let h = Harness::new(pool, &clock, vec![flaky.clone()]);
    let run = h
        .start(
            json!({
                "schema_version": 1,
                "id": "deliver",
                "steps": [{
                    "id": "send",
                    "action": "test.flaky",
                    "params": { "to": "ada@example.com" },
                    "retry": { "max_attempts": 3, "backoff": "exponential", "initial": "10s" },
                }],
            }),
            json!({}),
        )
        .await;

    // Start the Run, then the first Attempt, which fails after 2 s.
    let advanced = h.drive().await;
    assert_eq!(advanced.len(), 2);
    assert_eq!(advanced[1].status, RunStatus::Running);
    assert_eq!(advanced[1].next_job_at, Some(t0() + TimeDelta::seconds(12)));

    // The retry waits out its backoff: 10 s, then 20 s.
    h.clock.advance(TimeDelta::seconds(9));
    assert!(h.drive().await.is_empty());
    h.clock.advance(TimeDelta::seconds(1));
    let advanced = h.drive().await;
    assert_eq!(advanced.len(), 1);
    assert_eq!(advanced[0].next_job_at, Some(t0() + TimeDelta::seconds(34)));
    h.clock.advance(TimeDelta::seconds(19));
    assert!(h.drive().await.is_empty());
    h.clock.advance(TimeDelta::seconds(1));
    let advanced = h.drive().await;
    assert_eq!(advanced.len(), 1);
    assert_eq!(advanced[0].status, RunStatus::Succeeded);

    let calls = flaky.calls();
    let key = format!("{run}:send:");
    assert_eq!(keys(&calls), [key.clone(), key.clone(), key]);
    let attempts: Vec<_> = calls.iter().map(|call| call.context.attempt).collect();
    assert_eq!(attempts, [1, 2, 3]);

    let timeline = h.timeline(run).await;
    let summary: Vec<_> = timeline
        .iter()
        .map(|entry| (attempt(entry), entry.status, entry.duration()))
        .collect();
    assert_eq!(
        summary,
        [
            (1, AttemptStatus::Failed, TimeDelta::seconds(2)),
            (2, AttemptStatus::Failed, TimeDelta::seconds(2)),
            (3, AttemptStatus::Succeeded, TimeDelta::seconds(2)),
        ]
    );
    assert_eq!(
        timeline[0].error,
        Some(json!({ "class": "retryable", "message": "upstream returned 503" }))
    );
    assert_eq!(timeline[0].input, Some(json!({ "to": "ada@example.com" })));
    assert_eq!(timeline[2].output, Some(json!({ "delivered": true })));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_run_fails_once_a_retryable_step_runs_out_of_attempts(pool: PgPool) {
    let failing =
        Arc::new(FakeAction::new("test.down").failing(ActionError::retryable("connection reset")));
    let h = Harness::new(pool, &ManualClock::new(t0()), vec![failing.clone()]);
    let run = h
        .start(
            json!({
                "schema_version": 1,
                "id": "deliver",
                "steps": [{
                    "id": "send",
                    "action": "test.down",
                    "retry": { "max_attempts": 2, "backoff": "fixed", "initial": "5s" },
                }],
            }),
            json!({}),
        )
        .await;

    h.drive().await;
    h.clock.advance(TimeDelta::seconds(5));
    let advanced = h.drive().await;

    assert_eq!(advanced.len(), 1);
    assert_eq!(advanced[0].status, RunStatus::Failed);
    assert_eq!(advanced[0].next_job_at, None);
    assert_eq!(failing.calls().len(), 2);
    let failed = h.run(run).await;
    assert_eq!(failed.status, RunStatus::Failed);
    assert_eq!(
        failed.checkpoint.cursor,
        [json!("steps"), json!(0)],
        "a failed Run keeps its Cursor on the failed Step"
    );
    assert_eq!(h.jobs_left(run).await, 0);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_fatal_failure_fails_the_run_without_retrying(pool: PgPool) {
    let invalid = Arc::new(
        FakeAction::new("test.reject").failing(ActionError::fatal("422 Unprocessable Entity")),
    );
    let after = Arc::new(FakeAction::new("test.after"));
    let h = Harness::new(
        pool,
        &ManualClock::new(t0()),
        vec![invalid.clone(), after.clone()],
    );
    let run = h
        .start(
            json!({
                "schema_version": 1,
                "id": "deliver",
                "steps": [
                    {
                        "id": "send",
                        "action": "test.reject",
                        "retry": { "max_attempts": 5, "initial": "1s" },
                    },
                    { "id": "after", "action": "test.after" },
                ],
            }),
            json!({}),
        )
        .await;

    let advanced = h.drive().await;

    assert_eq!(advanced.last().unwrap().status, RunStatus::Failed);
    assert_eq!(invalid.calls().len(), 1);
    assert!(after.calls().is_empty());
    let timeline = h.timeline(run).await;
    assert_eq!(timeline.len(), 1);
    assert_eq!(
        timeline[0].error,
        Some(json!({ "class": "fatal", "message": "422 Unprocessable Entity" }))
    );
    assert_eq!(h.jobs_left(run).await, 0);
}

/// Moves the Clock past the Step's timeout, then waits for cancellation.
#[derive(Debug)]
struct Hangs {
    inner: FakeAction,
    clock: ManualClock,
}

impl Action for Hangs {
    fn descriptor(&self) -> &ActionDescriptor {
        self.inner.descriptor()
    }

    fn execute(
        &self,
        call: ActionCall,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        self.clock.advance(TimeDelta::seconds(31));
        self.inner.execute(call, cancel)
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_attempt_past_its_timeout_is_cancelled_and_recorded_as_a_retryable_timeout(
    pool: PgPool,
) {
    let clock = ManualClock::new(t0());
    let hangs = Arc::new(Hangs {
        inner: FakeAction::new("test.hang").until_cancelled(),
        clock: clock.clone(),
    });
    let h = Harness::new(pool, &clock, vec![hangs.clone()]);
    let run = h
        .start(
            json!({
                "schema_version": 1,
                "id": "slow",
                "steps": [{
                    "id": "wait",
                    "action": "test.hang",
                    "timeout": "30s",
                    "retry": { "max_attempts": 2, "initial": "1s" },
                }],
            }),
            json!({}),
        )
        .await;

    let advanced = h.drive().await;

    assert!(hangs.inner.saw_cancellation());
    assert_eq!(
        advanced[1].status,
        RunStatus::Running,
        "a timeout is retryable, so the retry policy applies"
    );
    let timeline = h.timeline(run).await;
    assert_eq!(timeline[0].status, AttemptStatus::Failed);
    assert_eq!(
        timeline[0].error,
        Some(json!({
            "class": "retryable",
            "message": "Attempt timed out after 30s",
            "code": "timeout",
        }))
    );
    assert_eq!(timeline[0].duration(), TimeDelta::seconds(31));
}

/// Simulates a crash after the Action ran but before its transition committed: during its
/// first call, the Worker's Lease expires and another Worker claims the Job, so the first
/// Worker's commit is refused and rolls back.
struct LosesLease {
    descriptor: ActionDescriptor,
    pool: PgPool,
    clock: ManualClock,
    taken_over: Mutex<Option<Claimed>>,
    calls: Mutex<Vec<ActionCall>>,
}

impl std::fmt::Debug for LosesLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LosesLease").finish_non_exhaustive()
    }
}

impl Action for LosesLease {
    fn descriptor(&self) -> &ActionDescriptor {
        &self.descriptor
    }

    fn execute(
        &self,
        call: ActionCall,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        Box::pin(async move {
            let first = {
                let mut calls = self.calls.lock().unwrap();
                calls.push(call);
                calls.len() == 1
            };
            if first {
                self.clock.advance(LEASE + TimeDelta::seconds(1));
                let mut conn = self.pool.acquire().await.unwrap();
                let claimed = PgJobRepository
                    .claim(&mut conn, "worker-b", 1, LEASE, self.clock.now())
                    .await
                    .unwrap()
                    .pop();
                *self.taken_over.lock().unwrap() = claimed;
            }
            Ok(json!({ "charged": true }))
        })
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_crash_before_commit_reruns_the_step_with_the_same_key_and_completes_once(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let action = Arc::new(LosesLease {
        descriptor: descriptor("test.charge"),
        pool: pool.clone(),
        clock: clock.clone(),
        taken_over: Mutex::new(None),
        calls: Mutex::new(Vec::new()),
    });
    let h = Harness::new(pool, &clock, vec![action.clone()]);
    let run = h
        .start(
            json!({
                "schema_version": 1,
                "id": "billing",
                "steps": [{ "id": "charge", "action": "test.charge" }],
            }),
            json!({}),
        )
        .await;
    let started = h.claim("worker-a").await.unwrap();
    h.interpreter.advance(&started).await.unwrap();

    let first = h.claim("worker-a").await.unwrap();
    let crashed = h.interpreter.advance(&first).await;

    assert!(
        matches!(crashed, Err(EngineError::Store(StoreError::LeaseLost(_)))),
        "{crashed:?}"
    );
    let unchanged = h.run(run).await;
    assert_eq!(unchanged.status, RunStatus::Running);
    assert_eq!(unchanged.checkpoint.cursor, [json!("steps"), json!(0)]);
    assert!(h.timeline(run).await.is_empty());

    let second = action.taken_over.lock().unwrap().take().unwrap();
    let advanced = h.interpreter.advance(&second).await.unwrap();

    assert_eq!(advanced.status, RunStatus::Succeeded);
    let calls = action.calls.lock().unwrap().clone();
    let key = format!("{run}:charge:");
    assert_eq!(keys(&calls), [key.clone(), key]);
    let timeline = h.timeline(run).await;
    assert_eq!(timeline.len(), 1, "the Run completes once");
    assert_eq!(timeline[0].output, Some(json!({ "charged": true })));
    assert_eq!(h.jobs_left(run).await, 0);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn invalid_input_fails_the_run_before_any_step_runs(pool: PgPool) {
    let echo = Arc::new(FakeAction::new("test.echo"));
    let h = Harness::new(pool, &ManualClock::new(t0()), vec![echo.clone()]);
    let run = h
        .start(
            json!({
                "schema_version": 1,
                "id": "signup",
                "input_schema": {
                    "type": "object",
                    "required": ["email"],
                    "properties": { "email": { "type": "string" } },
                },
                "steps": [{ "id": "greet", "action": "test.echo" }],
            }),
            json!({ "name": "ada" }),
        )
        .await;

    let advanced = h.drive().await;

    assert_eq!(advanced.len(), 1);
    assert_eq!(advanced[0].status, RunStatus::Failed);
    assert!(echo.calls().is_empty());
    assert_eq!(h.run(run).await.status, RunStatus::Failed);
    let timeline = h.timeline(run).await;
    assert_eq!(timeline.len(), 1);
    assert_eq!(
        timeline[0].step, None,
        "the entry is about the Run, not a Step"
    );
    assert_eq!(timeline[0].status, AttemptStatus::Failed);
    assert_eq!(timeline[0].input, Some(json!({ "name": "ada" })));
    let error = timeline[0].error.as_ref().unwrap();
    assert_eq!(error["code"], "invalid_input");
    assert_eq!(error["class"], "fatal");
    assert_eq!(error["details"]["violations"][0]["path"], "");
    assert!(
        error["details"]["violations"][0]["message"]
            .as_str()
            .unwrap()
            .contains("email"),
        "{error}"
    );
}

#[derive(Debug, Default)]
struct Secrets(BTreeMap<String, String>);

impl SecretResolver for Secrets {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<String>, ActionError>> {
        Box::pin(async move { Ok(self.0.get(name).cloned()) })
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn secrets_reach_the_action_but_never_the_timeline(pool: PgPool) {
    let api = Arc::new(FakeAction::new("test.api").returning(json!({ "status": 200 })));
    let secrets = Secrets(BTreeMap::from([("API_TOKEN".into(), "s3cr3t".into())]));
    let h = Harness::new(pool, &ManualClock::new(t0()), vec![api.clone()]).with_secrets(secrets);
    let run = h
        .start(
            json!({
                "schema_version": 1,
                "id": "call-api",
                "steps": [{
                    "id": "call",
                    "action": "test.api",
                    "params": {
                        "headers": { "Authorization": "Bearer {{ secrets.API_TOKEN }}" },
                        "user": "{{ input.user }}",
                    },
                }],
            }),
            json!({ "user": 42 }),
        )
        .await;

    h.drive().await;

    assert_eq!(
        Value::Object(api.calls()[0].params.clone()),
        json!({ "headers": { "Authorization": "Bearer s3cr3t" }, "user": 42 })
    );
    let timeline = h.timeline(run).await;
    assert_eq!(
        timeline[0].input,
        Some(json!({
            "headers": { "Authorization": "Bearer {{ secrets.API_TOKEN }}" },
            "user": 42,
        })),
        "a Template reading a Secret is recorded as written"
    );
    let run = h.run(run).await;
    assert!(!run.checkpoint.state.to_string().contains("s3cr3t"));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_unknown_action_fails_the_step_with_a_code(pool: PgPool) {
    let h = Harness::new(pool, &ManualClock::new(t0()), vec![]);
    let run = h
        .start(
            json!({
                "schema_version": 1,
                "id": "broken",
                "steps": [{
                    "id": "call",
                    "action": "test.missing",
                }],
            }),
            json!({}),
        )
        .await;

    h.drive().await;

    let timeline = h.timeline(run).await;
    assert_eq!(
        timeline[0].error.as_ref().unwrap()["code"],
        "unknown_action"
    );
    assert_eq!(h.run(run).await.status, RunStatus::Failed);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_undefined_secret_fails_the_step(pool: PgPool) {
    let api = Arc::new(FakeAction::new("test.api"));
    let h = Harness::new(pool, &ManualClock::new(t0()), vec![api.clone()]);
    let run = h
        .start(
            json!({
                "schema_version": 1,
                "id": "call-api",
                "steps": [{
                    "id": "call",
                    "action": "test.api",
                    "params": { "token": "{{ secrets.API_TOKEN }}" },
                }],
            }),
            json!({}),
        )
        .await;

    h.drive().await;

    assert!(api.calls().is_empty());
    let error = h.timeline(run).await[0].error.clone().unwrap();
    assert_eq!(error["code"], "unknown_secret");
    assert_eq!(error["class"], "fatal");
}
