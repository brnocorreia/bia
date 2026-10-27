//! Run retention deletes finished Runs with their Timelines and Jobs once they are past their
//! retention, keeps failed Runs on a retention of their own, never touches unfinished Runs, and
//! deletes dead letters on a retention of their own (ADR-0021, ADR-0041).

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use bia_core::action::CancellationToken;
use bia_core::clock::ManualClock;
use bia_core::dsl::{ActionName, Definition, QueueName, StepId, WorkflowId};
use bia_engine::{Chore, Retention, RetentionConfig, Scheduler, SchedulerConfig, start_run};
use bia_store::job::{JobRepository, PgJobRepository};
use bia_store::queue::{MessageFilter, PgQueueRepository, QueueRepository, QueueSettings};
use bia_store::run::{Checkpoint, PgRunRepository, RunId, RunRepository, RunStatus};
use bia_store::timeline::{
    AttemptStatus, NewTimelineEntry, PgTimelineRepository, StepAttempt, TimelineRepository,
};
use bia_store::version::{PgVersionRepository, VersionNumber, VersionRepository};
use bia_store::workflow::{PgWorkflowRepository, WorkflowRepository};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::json;
use sqlx::PgPool;

fn t0() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

fn days(n: i64) -> TimeDelta {
    TimeDelta::days(n)
}

struct Harness {
    pool: PgPool,
    version: VersionNumber,
}

impl Harness {
    async fn new(pool: PgPool) -> Self {
        let definition = Definition::from_value(json!({
            "schema_version": 1,
            "id": "report",
            "steps": [{ "id": "done", "action": "flow.succeed" }],
        }))
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        PgWorkflowRepository
            .create(&mut conn, &definition.id)
            .await
            .unwrap();
        let draft = PgVersionRepository
            .create_draft(&mut conn, &definition)
            .await
            .unwrap();
        PgVersionRepository
            .publish(&mut conn, &definition.id, draft.number, t0())
            .await
            .unwrap();
        Self {
            pool,
            version: draft.number,
        }
    }

    /// A Run started at `t0`, with one Timeline entry and its first Job, that moved to
    /// `status` at `at`.
    async fn run(&self, status: RunStatus, at: DateTime<Utc>) -> RunId {
        let mut conn = self.pool.acquire().await.unwrap();
        let run = start_run(
            &mut conn,
            &WorkflowId::new("report").unwrap(),
            self.version,
            json!({}),
            json!({ "type": "manual" }),
            t0(),
        )
        .await
        .unwrap();
        PgTimelineRepository
            .append(
                &mut conn,
                run.id,
                &NewTimelineEntry {
                    step: Some(StepAttempt {
                        step_id: StepId::new("done").unwrap(),
                        iteration_path: String::new(),
                        attempt: 1,
                        action: ActionName::new("flow.succeed").unwrap(),
                    }),
                    status: AttemptStatus::Succeeded,
                    input: Some(json!({})),
                    output: Some(json!({})),
                    error: None,
                    started_at: t0(),
                    finished_at: t0(),
                },
            )
            .await
            .unwrap();
        if status != RunStatus::Pending {
            PgRunRepository
                .update_checkpoint(&mut conn, run.id, status, &Checkpoint::default(), at)
                .await
                .unwrap();
        }
        run.id
    }

    async fn exists(&self, run: RunId) -> bool {
        let mut conn = self.pool.acquire().await.unwrap();
        PgRunRepository.get(&mut conn, run).await.unwrap().is_some()
    }

    /// Whether the Run is gone along with its Timeline and Jobs.
    async fn deleted(&self, run: RunId) -> bool {
        let mut conn = self.pool.acquire().await.unwrap();
        let timeline = PgTimelineRepository.list(&mut conn, run).await.unwrap();
        let jobs = PgJobRepository.list_for_run(&mut conn, run).await.unwrap();
        let gone = !self.exists(run).await;
        if gone {
            assert!(timeline.is_empty(), "the Timeline of run {run} remains");
            assert!(jobs.is_empty(), "the Jobs of run {run} remain");
        } else {
            assert!(!timeline.is_empty(), "run {run} lost its Timeline");
        }
        gone
    }

    async fn retain(&self, config: RetentionConfig, now: DateTime<Utc>) {
        Retention::new(config).run(&self.pool, now).await.unwrap();
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_run_finished_more_than_90_days_ago_is_deleted_with_its_timeline_and_jobs(pool: PgPool) {
    let h = Harness::new(pool).await;
    let old = h.run(RunStatus::Succeeded, t0()).await;
    let recent = h.run(RunStatus::Succeeded, t0() + days(2)).await;

    h.retain(RetentionConfig::default(), t0() + days(91)).await;

    assert!(h.deleted(old).await);
    assert!(!h.deleted(recent).await, "it finished 89 days ago");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn cancelled_runs_follow_the_finished_retention_and_failed_runs_their_own(pool: PgPool) {
    let h = Harness::new(pool).await;
    let succeeded = h.run(RunStatus::Succeeded, t0()).await;
    let cancelled = h.run(RunStatus::Cancelled, t0()).await;
    let failed = h.run(RunStatus::Failed, t0()).await;

    let config = RetentionConfig {
        finished_runs: days(30),
        failed_runs: days(180),
        ..RetentionConfig::default()
    };
    h.retain(config.clone(), t0() + days(31)).await;
    assert!(h.deleted(succeeded).await);
    assert!(h.deleted(cancelled).await);
    assert!(!h.deleted(failed).await);

    h.retain(config, t0() + days(181)).await;
    assert!(h.deleted(failed).await);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn failed_runs_can_be_kept_for_less_time_than_finished_ones(pool: PgPool) {
    let h = Harness::new(pool).await;
    let succeeded = h.run(RunStatus::Succeeded, t0()).await;
    let failed = h.run(RunStatus::Failed, t0()).await;

    let config = RetentionConfig {
        failed_runs: days(7),
        ..RetentionConfig::default()
    };
    h.retain(config, t0() + days(8)).await;

    assert!(!h.deleted(succeeded).await);
    assert!(h.deleted(failed).await);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn unfinished_runs_are_never_deleted_however_old(pool: PgPool) {
    let h = Harness::new(pool).await;
    let mut unfinished = Vec::new();
    for status in [
        RunStatus::Pending,
        RunStatus::Running,
        RunStatus::Sleeping,
        RunStatus::Waiting,
    ] {
        unfinished.push(h.run(status, t0()).await);
    }

    h.retain(RetentionConfig::default(), t0() + days(10_000))
        .await;

    for run in unfinished {
        assert!(!h.deleted(run).await);
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_backlog_is_deleted_in_batches_over_several_runs(pool: PgPool) {
    let h = Harness::new(pool).await;
    let mut runs = Vec::new();
    for day in 0..5 {
        runs.push(h.run(RunStatus::Succeeded, t0() + days(day)).await);
    }
    let config = RetentionConfig {
        batch_size: 2,
        batches_per_run: 2,
        ..RetentionConfig::default()
    };

    h.retain(config.clone(), t0() + days(100)).await;
    let mut remaining = Vec::new();
    for &run in &runs {
        if !h.deleted(run).await {
            remaining.push(run);
        }
    }
    assert_eq!(remaining, [runs[4]], "the oldest go first, four per run");

    h.retain(config, t0() + days(100)).await;
    assert!(h.deleted(runs[4]).await);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn dead_letters_are_deleted_once_past_their_own_retention(pool: PgPool) {
    let queue = |name| QueueName::new(name).unwrap();
    let queues = PgQueueRepository;
    let mut conn = pool.acquire().await.unwrap();
    let no_retries = QueueSettings {
        max_retries: 0,
        ..QueueSettings::default()
    };
    for name in ["emails-dlq", "emails"] {
        let settings = QueueSettings {
            dead_letter: (name == "emails").then(|| queue("emails-dlq")),
            ..no_retries.clone()
        };
        queues
            .put(&mut conn, &queue(name), &settings, t0())
            .await
            .unwrap();
    }
    let bodies = [json!("dead on day 0"), json!("dead on day 5")];
    for (day, body) in [0, 5].into_iter().zip(bodies) {
        let at = t0() + days(day);
        queues
            .publish(&mut conn, &queue("emails"), &[body], at, at)
            .await
            .unwrap();
        let delivered = queues
            .claim(&mut conn, &queue("emails"), "consumer", 10, at)
            .await
            .unwrap();
        let backoff = bia_store::job::Backoff::default();
        queues
            .nack(&mut conn, &delivered[0].lease, "bounced", &backoff, at)
            .await
            .unwrap();
    }
    queues
        .publish(&mut conn, &queue("emails"), &[json!("waiting")], t0(), t0())
        .await
        .unwrap();
    let left = |queue_name| MessageFilter {
        queue: queue(queue_name),
        dead_letters: None,
        after: None,
        limit: 10,
    };
    let config = RetentionConfig {
        dead_letters: days(14),
        ..RetentionConfig::default()
    };

    Retention::new(config)
        .run(&pool, t0() + days(15))
        .await
        .unwrap();

    let dead = queues
        .list_messages(&mut conn, &left("emails-dlq"))
        .await
        .unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].body, json!("dead on day 5"));
    let waiting = queues
        .list_messages(&mut conn, &left("emails"))
        .await
        .unwrap();
    assert_eq!(
        waiting.len(),
        1,
        "a Message that is not a dead letter is kept"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_scheduler_runs_retention_once_the_clock_passes_90_days(pool: PgPool) {
    let h = Harness::new(pool).await;
    let run = h.run(RunStatus::Succeeded, t0()).await;
    let clock = ManualClock::new(t0());
    let shutdown = CancellationToken::new();
    let schedulers: Vec<_> = ["scheduler-a", "scheduler-b"]
        .into_iter()
        .map(|id| {
            let scheduler = Scheduler::new(
                id,
                h.pool.clone(),
                Arc::new(clock.clone()),
                SchedulerConfig::default(),
            )
            .with_chore(Arc::new(Retention::default()));
            let shutdown = shutdown.clone();
            tokio::spawn(async move { scheduler.run(shutdown).await.unwrap() })
        })
        .collect();

    clock.set(t0() + days(89));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.exists(run).await, "it is 89 days old");

    clock.set(t0() + days(90) + TimeDelta::minutes(10));
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        while h.exists(run).await {
            clock.advance(TimeDelta::seconds(1));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "retention never deleted the run");
    assert!(h.deleted(run).await);

    shutdown.cancel();
    for scheduler in schedulers {
        scheduler.await.unwrap();
    }
}
