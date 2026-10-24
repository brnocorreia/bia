//! The scheduler fires cron Triggers of Active Versions on time, in their time zone and with
//! jitter, honours the Overlap and Catch-up Policies, and fires each occurrence exactly once
//! across scheduler replicas, including when the leader dies (ADR-0039, ADR-0040).

#![allow(clippy::unwrap_used)]

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bia_core::action::{BoxFuture, CancellationToken};
use bia_core::clock::{Clock, ManualClock};
use bia_core::dsl::{Definition, WorkflowId};
use bia_engine::{CATCH_UP_LIMIT, Chore, EngineError, Scheduler, SchedulerConfig};
use bia_store::run::{
    Checkpoint, PgRunRepository, Run, RunFilter, RunId, RunRepository, RunStatus,
};
use bia_store::schedule::{PgScheduleRepository, ScheduleRepository};
use bia_store::version::{PgVersionRepository, VersionNumber, VersionRepository};
use bia_store::workflow::{PgWorkflowRepository, WorkflowRepository};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::task::JoinHandle;

const POLL: TimeDelta = TimeDelta::seconds(1);

fn at(rfc3339: &str) -> DateTime<Utc> {
    rfc3339.parse().unwrap()
}

struct Harness {
    pool: PgPool,
    clock: ManualClock,
}

struct Running {
    shutdown: CancellationToken,
    handle: JoinHandle<()>,
}

impl Running {
    async fn shut_down(self) {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(20), self.handle)
            .await
            .expect("the scheduler did not stop")
            .unwrap();
    }

    /// Stops the scheduler the way a crash would: no release, no goodbye.
    fn kill(self) {
        self.handle.abort();
    }
}

impl Harness {
    fn new(pool: PgPool, start: &str) -> Self {
        Self {
            pool,
            clock: ManualClock::new(at(start)),
        }
    }

    /// Publishes a Version of `workflow` with these Triggers, creating the Workflow first if
    /// needed, and makes it the Active Version.
    async fn activate(&self, workflow: &str, triggers: Value) -> VersionNumber {
        let definition = Definition::from_value(json!({
            "schema_version": 1,
            "id": workflow,
            "triggers": triggers,
            "steps": [{ "id": "done", "action": "flow.succeed" }],
        }))
        .unwrap();
        let mut conn = self.pool.acquire().await.unwrap();
        if PgWorkflowRepository
            .get(&mut conn, &definition.id)
            .await
            .unwrap()
            .is_none()
        {
            PgWorkflowRepository
                .create(&mut conn, &definition.id)
                .await
                .unwrap();
        }
        let draft = PgVersionRepository
            .create_draft(&mut conn, &definition)
            .await
            .unwrap();
        PgVersionRepository
            .publish(&mut conn, &definition.id, draft.number, self.clock.now())
            .await
            .unwrap();
        PgWorkflowRepository
            .set_active_version(&mut conn, &definition.id, draft.number)
            .await
            .unwrap();
        draft.number
    }

    fn scheduler(&self, id: &str) -> Scheduler {
        Scheduler::new(
            id,
            self.pool.clone(),
            Arc::new(self.clock.clone()),
            SchedulerConfig {
                poll_interval: POLL,
            },
        )
    }

    fn start(&self, scheduler: Scheduler) -> Running {
        let shutdown = CancellationToken::new();
        let token = shutdown.clone();
        let handle = tokio::spawn(async move { scheduler.run(token).await.unwrap() });
        Running { shutdown, handle }
    }

    /// Moves the Clock to `to` and lets `scheduler` handle that moment.
    async fn tick_at(&self, scheduler: &Scheduler, to: &str) -> Vec<RunId> {
        self.clock.set(at(to));
        scheduler.tick().await.unwrap()
    }

    /// The Workflow's Runs, ordered by the occurrence they are for.
    async fn runs(&self, workflow: &str) -> Vec<Run> {
        let mut conn = self.pool.acquire().await.unwrap();
        let mut runs = PgRunRepository
            .list(
                &mut conn,
                &RunFilter {
                    workflow_id: Some(WorkflowId::new(workflow).unwrap()),
                    limit: 1_000,
                    ..RunFilter::default()
                },
            )
            .await
            .unwrap();
        runs.sort_by_key(scheduled_at);
        runs
    }

    async fn scheduled(&self, workflow: &str) -> Vec<String> {
        self.runs(workflow).await.iter().map(scheduled_at).collect()
    }

    async fn finish(&self, run: RunId) {
        let mut conn = self.pool.acquire().await.unwrap();
        PgRunRepository
            .update_checkpoint(
                &mut conn,
                run,
                RunStatus::Succeeded,
                &Checkpoint::default(),
                self.clock.now(),
            )
            .await
            .unwrap();
    }

    /// Moves the Clock forward a poll at a time until `check` holds, so running schedulers
    /// keep polling while a test waits on them.
    async fn advance_until<F, Fut>(&self, what: &str, mut check: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        let waited = tokio::time::timeout(Duration::from_secs(20), async {
            while !check().await {
                self.clock.advance(POLL);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(waited.is_ok(), "timed out waiting for {what}");
    }
}

fn scheduled_at(run: &Run) -> String {
    run.trigger["scheduled_at"].as_str().unwrap().to_owned()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_cron_trigger_fires_at_each_occurrence_in_its_time_zone_across_a_dst_change(
    pool: PgPool,
) {
    let h = Harness::new(pool, "2026-03-06T12:00:00Z");
    h.activate(
        "report",
        json!([{ "id": "daily", "type": "cron", "expr": "0 9 * * *", "tz": "America/New_York" }]),
    )
    .await;
    let scheduler = h.scheduler("scheduler-a");

    // 09:00 in New York is 14:00 UTC until DST starts on 8 March, then 13:00 UTC.
    let occurrences = [
        ("2026-03-06T13:59:59Z", "2026-03-06T14:00:00Z"),
        ("2026-03-07T13:59:59Z", "2026-03-07T14:00:00Z"),
        ("2026-03-08T12:59:59Z", "2026-03-08T13:00:00Z"),
        ("2026-03-09T12:59:59Z", "2026-03-09T13:00:00Z"),
    ];
    for (before, on_time) in occurrences {
        assert!(h.tick_at(&scheduler, before).await.is_empty(), "{before}");
        let started = h.tick_at(&scheduler, on_time).await;
        assert_eq!(started.len(), 1, "{on_time}");
        if on_time != occurrences[3].1 {
            // Otherwise `overlap: skip` would skip the next occurrence.
            h.finish(started[0]).await;
        }
    }

    let runs = h.runs("report").await;
    assert_eq!(
        runs.iter().map(scheduled_at).collect::<Vec<_>>(),
        occurrences.map(|(_, on_time)| on_time)
    );
    assert_eq!(
        runs[2].trigger,
        json!({ "type": "cron", "id": "daily", "scheduled_at": "2026-03-08T13:00:00Z" })
    );
    assert_eq!(runs[2].created_at, at("2026-03-08T13:00:00Z"));
    assert_eq!(runs[3].status, RunStatus::Pending, "it waits for a Worker");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn concurrent_ticks_fire_each_occurrence_exactly_once(pool: PgPool) {
    const WORKFLOWS: usize = 10;
    const SCHEDULERS: usize = 6;
    let h = Harness::new(pool, "2026-01-01T08:30:00Z");
    for i in 0..WORKFLOWS {
        h.activate(
            &format!("wf-{i}"),
            json!([{ "id": "hourly", "type": "cron", "expr": "0 * * * *", "overlap": "allow" }]),
        )
        .await;
    }
    h.scheduler("setup").tick().await.unwrap();

    for hour in ["09", "10", "11"] {
        h.clock.set(at(&format!("2026-01-01T{hour}:00:00Z")));
        let ticks: Vec<_> = (0..SCHEDULERS)
            .map(|i| {
                let scheduler = h.scheduler(&format!("scheduler-{i}"));
                tokio::spawn(async move { scheduler.tick().await.unwrap() })
            })
            .collect();
        let mut started = 0;
        for tick in ticks {
            started += tick.await.unwrap().len();
        }
        assert_eq!(started, WORKFLOWS, "at {hour}:00");
    }

    for i in 0..WORKFLOWS {
        assert_eq!(
            h.scheduled(&format!("wf-{i}")).await,
            [
                "2026-01-01T09:00:00Z",
                "2026-01-01T10:00:00Z",
                "2026-01-01T11:00:00Z"
            ]
        );
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn two_running_schedulers_fire_each_occurrence_exactly_once(pool: PgPool) {
    let h = Harness::new(pool, "2026-01-01T08:59:30Z");
    h.activate(
        "ping",
        json!([{ "id": "minutely", "type": "cron", "expr": "* * * * *", "overlap": "allow" }]),
    )
    .await;
    let a = h.start(h.scheduler("scheduler-a"));
    let b = h.start(h.scheduler("scheduler-b"));

    for fired in 1..=5 {
        h.advance_until(&format!("{fired} runs"), || async {
            h.runs("ping").await.len() >= fired
        })
        .await;
    }
    a.shut_down().await;
    b.shut_down().await;

    let scheduled = h.scheduled("ping").await;
    let last = h.clock.now().format("%Y-%m-%dT%H:%M:00Z").to_string();
    let expected: Vec<String> = (0..scheduled.len())
        .map(|minute| {
            (at("2026-01-01T09:00:00Z") + TimeDelta::minutes(i64::try_from(minute).unwrap()))
                .format("%Y-%m-%dT%H:%M:00Z")
                .to_string()
        })
        .collect();
    assert_eq!(scheduled, expected, "each minute fired once, none twice");
    assert_eq!(scheduled.last(), Some(&last));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn when_the_leader_dies_another_scheduler_takes_over_without_duplicates(pool: PgPool) {
    let h = Harness::new(pool, "2026-01-01T08:59:30Z");
    h.activate(
        "ping",
        json!([{ "id": "minutely", "type": "cron", "expr": "* * * * *", "overlap": "allow" }]),
    )
    .await;
    let leader = h.start(h.scheduler("scheduler-a"));
    h.advance_until("the first run", || async {
        h.runs("ping").await.len() == 1
    })
    .await;
    let follower = h.start(h.scheduler("scheduler-b"));
    h.advance_until("the second run", || async {
        h.runs("ping").await.len() == 2
    })
    .await;

    leader.kill();
    for fired in 3..=5 {
        h.advance_until(&format!("{fired} runs"), || async {
            h.runs("ping").await.len() >= fired
        })
        .await;
    }
    follower.shut_down().await;

    let scheduled = h.scheduled("ping").await;
    let mut distinct = scheduled.clone();
    distinct.dedup();
    assert_eq!(distinct, scheduled, "no occurrence fired twice");
    assert_eq!(scheduled[0], "2026-01-01T09:00:00Z");
    assert!(scheduled.len() >= 5);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn skip_starts_no_run_while_the_previous_one_is_active(pool: PgPool) {
    let h = Harness::new(pool, "2026-01-01T08:00:00Z");
    h.activate(
        "sync",
        json!([{ "id": "hourly", "type": "cron", "expr": "0 * * * *", "overlap": "skip" }]),
    )
    .await;
    let s = h.scheduler("scheduler-a");
    h.tick_at(&s, "2026-01-01T08:00:00Z").await;

    let first = h.tick_at(&s, "2026-01-01T09:00:00Z").await;
    assert!(h.tick_at(&s, "2026-01-01T10:00:00Z").await.is_empty());
    h.finish(first[0]).await;
    assert!(
        h.tick_at(&s, "2026-01-01T10:00:05Z").await.is_empty(),
        "a skipped occurrence stays skipped"
    );
    assert_eq!(h.tick_at(&s, "2026-01-01T11:00:00Z").await.len(), 1);

    assert_eq!(
        h.scheduled("sync").await,
        ["2026-01-01T09:00:00Z", "2026-01-01T11:00:00Z"]
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn queue_starts_the_held_back_run_once_the_previous_one_finishes(pool: PgPool) {
    let h = Harness::new(pool, "2026-01-01T08:00:00Z");
    h.activate(
        "sync",
        json!([{ "id": "hourly", "type": "cron", "expr": "0 * * * *", "overlap": "queue" }]),
    )
    .await;
    let s = h.scheduler("scheduler-a");
    h.tick_at(&s, "2026-01-01T08:00:00Z").await;

    let first = h.tick_at(&s, "2026-01-01T09:00:00Z").await;
    assert!(h.tick_at(&s, "2026-01-01T10:00:00Z").await.is_empty());
    assert!(h.tick_at(&s, "2026-01-01T10:30:00Z").await.is_empty());
    h.finish(first[0]).await;
    let queued = h.tick_at(&s, "2026-01-01T10:30:01Z").await;

    assert_eq!(queued.len(), 1);
    let runs = h.runs("sync").await;
    assert_eq!(
        runs.iter().map(scheduled_at).collect::<Vec<_>>(),
        ["2026-01-01T09:00:00Z", "2026-01-01T10:00:00Z"]
    );
    assert_eq!(runs[1].created_at, at("2026-01-01T10:30:01Z"));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn allow_starts_a_run_while_the_previous_one_is_active(pool: PgPool) {
    let h = Harness::new(pool, "2026-01-01T08:00:00Z");
    h.activate(
        "sync",
        json!([{ "id": "hourly", "type": "cron", "expr": "0 * * * *", "overlap": "allow" }]),
    )
    .await;
    let s = h.scheduler("scheduler-a");
    h.tick_at(&s, "2026-01-01T08:00:00Z").await;

    h.tick_at(&s, "2026-01-01T09:00:00Z").await;
    assert_eq!(h.tick_at(&s, "2026-01-01T10:00:00Z").await.len(), 1);

    let runs = h.runs("sync").await;
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|run| !run.status.is_finished()));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn after_downtime_none_fires_nothing_latest_fires_once_and_all_fires_each_up_to_the_bound(
    pool: PgPool,
) {
    let h = Harness::new(pool, "2026-01-01T08:30:00Z");
    for catch_up in ["none", "latest", "all"] {
        h.activate(
            catch_up,
            json!([{
                "id": "hourly", "type": "cron", "expr": "0 * * * *",
                "overlap": "allow", "catch_up": catch_up,
            }]),
        )
        .await;
    }
    h.activate(
        "bounded",
        json!([{
            "id": "minutely", "type": "cron", "expr": "* * * * *",
            "overlap": "allow", "catch_up": "all",
        }]),
    )
    .await;
    let s = h.scheduler("scheduler-a");
    h.tick_at(&s, "2026-01-01T08:30:00Z").await;

    // Down from 08:30 until 13:30: 09:00 to 13:00 were missed.
    h.tick_at(&s, "2026-01-01T13:30:00Z").await;

    assert!(h.scheduled("none").await.is_empty());
    assert_eq!(h.scheduled("latest").await, ["2026-01-01T13:00:00Z"]);
    assert_eq!(
        h.scheduled("all").await,
        [
            "2026-01-01T09:00:00Z",
            "2026-01-01T10:00:00Z",
            "2026-01-01T11:00:00Z",
            "2026-01-01T12:00:00Z",
            "2026-01-01T13:00:00Z",
        ]
    );
    let bounded = h.scheduled("bounded").await;
    assert_eq!(bounded.len(), CATCH_UP_LIMIT, "300 were missed");
    assert_eq!(bounded.first().unwrap(), "2026-01-01T11:51:00Z");
    assert_eq!(bounded.last().unwrap(), "2026-01-01T13:30:00Z");

    h.tick_at(&s, "2026-01-01T14:00:00Z").await;
    assert_eq!(
        h.scheduled("none").await,
        ["2026-01-01T14:00:00Z"],
        "on time again"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn activating_a_new_version_replaces_its_schedules_and_the_old_one_never_fires(pool: PgPool) {
    let h = Harness::new(pool, "2026-01-01T08:00:00Z");
    h.activate(
        "digest",
        json!([{ "id": "morning", "type": "cron", "expr": "0 9 * * *" }]),
    )
    .await;
    let s = h.scheduler("scheduler-a");
    h.tick_at(&s, "2026-01-01T08:00:00Z").await;

    let v2 = h
        .activate(
            "digest",
            json!([{ "id": "evening", "type": "cron", "expr": "0 18 * * *" }]),
        )
        .await;
    let workflow = WorkflowId::new("digest").unwrap();
    let mut tx = h.pool.begin().await.unwrap();
    let claimed = PgScheduleRepository
        .claim_due(&mut tx, at("2026-01-01T09:00:00Z"))
        .await
        .unwrap();
    assert!(
        claimed.is_none(),
        "the old Version's Schedule cannot fire even before it is replaced"
    );
    drop(tx);

    assert!(h.tick_at(&s, "2026-01-01T09:00:00Z").await.is_empty());
    let mut conn = h.pool.acquire().await.unwrap();
    let schedules = PgScheduleRepository
        .list_for_workflow(&mut conn, &workflow)
        .await
        .unwrap();
    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0].trigger_id.as_str(), "evening");
    assert_eq!(schedules[0].version, v2);

    assert_eq!(h.tick_at(&s, "2026-01-01T18:00:00Z").await.len(), 1);
    let runs = h.runs("digest").await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].version, v2);
    assert_eq!(runs[0].trigger["id"], "evening");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_new_version_with_the_same_timing_keeps_the_next_occurrence_and_the_previous_run(
    pool: PgPool,
) {
    let h = Harness::new(pool, "2026-01-01T08:00:00Z");
    let daily = json!([{ "id": "daily", "type": "cron", "expr": "0 9 * * *" }]);
    h.activate("digest", daily.clone()).await;
    let s = h.scheduler("scheduler-a");
    h.tick_at(&s, "2026-01-01T08:00:00Z").await;
    h.tick_at(&s, "2026-01-01T09:00:00Z").await;

    h.activate("digest", daily).await;
    h.tick_at(&s, "2026-01-01T10:00:00Z").await;
    assert!(
        h.tick_at(&s, "2026-01-02T09:00:00Z").await.is_empty(),
        "`skip` still sees the Run the previous Version started"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn jitter_delays_each_firing_within_its_window(pool: PgPool) {
    let h = Harness::new(pool, "2026-01-01T00:30:00Z");
    h.activate(
        "spread",
        json!([{
            "id": "hourly", "type": "cron", "expr": "0 * * * *",
            "jitter": "10m", "overlap": "allow",
        }]),
    )
    .await;
    let s = h.scheduler("scheduler-a");
    h.tick_at(&s, "2026-01-01T00:30:00Z").await;

    for hour in 1..=12 {
        let occurrence = at("2026-01-01T00:00:00Z") + TimeDelta::hours(hour);
        for minute in 0..=10 {
            h.clock.set(occurrence + TimeDelta::minutes(minute));
            s.tick().await.unwrap();
        }
    }

    let runs = h.runs("spread").await;
    assert_eq!(runs.len(), 12, "each occurrence fired once");
    let delays: Vec<TimeDelta> = runs
        .iter()
        .map(|run| run.created_at - at(&scheduled_at(run)))
        .collect();
    assert!(
        delays
            .iter()
            .all(|delay| *delay >= TimeDelta::zero() && *delay <= TimeDelta::minutes(10)),
        "{delays:?}"
    );
    assert!(
        delays.iter().any(|delay| *delay != delays[0]),
        "firings spread out: {delays:?}"
    );
}

#[derive(Debug, Default)]
struct Counting {
    runs: AtomicUsize,
}

impl Chore for Counting {
    fn name(&self) -> &str {
        "counting"
    }

    fn interval(&self) -> TimeDelta {
        TimeDelta::hours(1)
    }

    fn run<'a>(
        &'a self,
        _pool: &'a PgPool,
        _now: DateTime<Utc>,
    ) -> BoxFuture<'a, Result<(), EngineError>> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn only_the_leader_runs_chores_and_a_stopping_leader_hands_over(pool: PgPool) {
    let h = Harness::new(pool, "2026-01-01T00:00:00Z");
    let chore = Arc::new(Counting::default());
    let count = || chore.runs.load(Ordering::SeqCst);
    let leader = h.start(h.scheduler("scheduler-a").with_chore(chore.clone()));
    h.advance_until("the chore to run on leadership", || async { count() == 1 })
        .await;
    let follower = h.start(h.scheduler("scheduler-b").with_chore(chore.clone()));

    h.clock.advance(TimeDelta::minutes(59));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(count(), 1, "neither runs it before its interval");
    h.advance_until("the chore's next run", || async { count() == 2 })
        .await;

    leader.shut_down().await;
    h.advance_until("the new leader to run the chore", || async { count() == 3 })
        .await;
    follower.shut_down().await;
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_workflow_without_an_active_version_has_no_schedules(pool: PgPool) {
    let h = Harness::new(pool, "2026-01-01T08:00:00Z");
    let mut conn = h.pool.acquire().await.unwrap();
    let workflow = WorkflowId::new("draft-only").unwrap();
    PgWorkflowRepository
        .create(&mut conn, &workflow)
        .await
        .unwrap();
    let definition = Definition::from_value(json!({
        "schema_version": 1,
        "id": "draft-only",
        "triggers": [{ "id": "daily", "type": "cron", "expr": "0 9 * * *" }],
        "steps": [],
    }))
    .unwrap();
    PgVersionRepository
        .create_draft(&mut conn, &definition)
        .await
        .unwrap();

    let s = h.scheduler("scheduler-a");
    h.tick_at(&s, "2026-01-01T08:00:00Z").await;
    assert!(h.tick_at(&s, "2026-01-01T09:00:00Z").await.is_empty());
    assert!(
        PgScheduleRepository
            .list_for_workflow(&mut conn, &workflow)
            .await
            .unwrap()
            .is_empty()
    );
}
