//! Workers claim Jobs and run them to completion, share a database without running a Step
//! twice, shut down without stranding Jobs, and stop in-flight Actions of cancelled Runs
//! (ADR-0031, ADR-0036).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::sync::Arc;

use bia_core::action::testing::FakeAction;
use bia_core::clock::Clock;
use bia_engine::{WorkerConfig, cancel_run};
use bia_store::job::{JobRepository, PgJobRepository};
use bia_store::run::RunStatus;
use bia_store::timeline::AttemptStatus;
use chrono::TimeDelta;
use serde_json::json;
use sqlx::PgPool;

mod common;

use common::{Gate, Harness, LEASE, config, eventually, ran, t0};

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_worker_runs_a_seeded_run_to_completion(pool: PgPool) {
    let echo = Arc::new(FakeAction::new("test.echo"));
    let mut h = Harness::new(pool, vec![echo.clone()]);
    h.publish(
        "greet",
        json!([
            { "id": "hello", "action": "test.echo", "params": { "n": 1 } },
            { "id": "bye", "action": "test.echo", "params": { "n": "{{ steps.hello.output.n + 1 }}" } },
        ]),
    )
    .await;
    let run = h.start_run().await;

    let worker = h.worker("worker-a", config());
    h.wait_for(run, RunStatus::Succeeded).await;
    worker.shut_down().await;

    let timeline = h.timeline(run).await;
    assert_eq!(
        ran(&timeline),
        [
            (Some("hello"), AttemptStatus::Succeeded),
            (Some("bye"), AttemptStatus::Succeeded),
        ]
    );
    assert_eq!(timeline[1].output, Some(json!({ "n": 2 })));
    assert!(h.jobs(run).await.is_empty());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn two_workers_process_a_batch_of_runs_without_running_any_step_twice(pool: PgPool) {
    const RUNS: usize = 20;
    let echo = Arc::new(FakeAction::new("test.echo"));
    let mut h = Harness::new(pool, vec![echo.clone()]);
    h.publish(
        "batch",
        json!([
            { "id": "one", "action": "test.echo" },
            { "id": "two", "action": "test.echo" },
            { "id": "three", "action": "test.echo" },
        ]),
    )
    .await;
    let a = h.worker("worker-a", config());
    let b = h.worker("worker-b", config());
    let mut runs = Vec::new();
    for _ in 0..RUNS {
        runs.push(h.start_run().await);
    }

    for &run in &runs {
        h.wait_for(run, RunStatus::Succeeded).await;
    }
    a.shut_down().await;
    b.shut_down().await;

    let keys: Vec<String> = echo
        .calls()
        .iter()
        .map(|call| call.idempotency_key.as_str().to_owned())
        .collect();
    let distinct: BTreeSet<&String> = keys.iter().collect();
    assert_eq!(keys.len(), RUNS * 3, "every Step ran");
    assert_eq!(distinct.len(), keys.len(), "no Step ran twice");
    for run in runs {
        let timeline = h.timeline(run).await;
        assert_eq!(timeline.len(), 3);
        assert!(
            timeline
                .iter()
                .all(|entry| entry.step.as_ref().unwrap().attempt == 1)
        );
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn shutdown_lets_an_in_flight_job_finish_within_the_grace_period(pool: PgPool) {
    let gate = Gate::new();
    let mut h = Harness::new(pool, vec![gate.clone()]);
    h.publish("slow", json!([{ "id": "wait", "action": "test.gate" }]))
        .await;
    let run = h.start_run().await;
    let worker = h.worker("worker-a", config());
    gate.wait_for_a_call().await;

    worker.shutdown.cancel();
    gate.open();
    worker.shut_down().await;

    assert_eq!(h.run(run).await.status, RunStatus::Succeeded);
    assert!(!gate.saw_cancellation());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn on_shutdown_a_worker_stops_claiming_and_releases_unfinished_jobs_to_another(pool: PgPool) {
    let gate = Gate::new();
    let mut h = Harness::new(pool, vec![gate.clone()]);
    h.publish("slow", json!([{ "id": "wait", "action": "test.gate" }]))
        .await;
    let unfinished = h.start_run().await;
    let quick_exit = WorkerConfig {
        shutdown_grace: TimeDelta::zero(),
        ..config()
    };
    let a = h.worker("worker-a", quick_exit);
    gate.wait_for_a_call().await;

    a.shutdown.cancel();
    let after_shutdown = h.start_run().await;
    a.shut_down().await;

    let released = h.jobs(unfinished).await.remove(0);
    assert_eq!(released.locked_by, None, "the Lease is released");
    assert_eq!(released.attempts, 1);
    let unclaimed = h.jobs(after_shutdown).await.remove(0);
    assert_eq!(
        unclaimed.attempts, 0,
        "a shutting-down Worker claims nothing"
    );

    gate.open();
    let b = h.worker("worker-b", config());
    h.wait_for(unfinished, RunStatus::Succeeded).await;
    h.wait_for(after_shutdown, RunStatus::Succeeded).await;
    b.shut_down().await;
    assert_eq!(
        h.clock.now(),
        t0(),
        "worker-b did not wait for the Lease to expire"
    );
    let timeline = h.timeline(unfinished).await;
    assert_eq!(ran(&timeline), [(Some("wait"), AttemptStatus::Succeeded)]);
    assert_eq!(
        gate.calls(),
        3,
        "the stopped call, then one per Run on worker-b"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn cancelling_a_run_cancels_its_in_flight_action_and_no_later_step_runs(pool: PgPool) {
    let gate = Gate::new();
    let echo = Arc::new(FakeAction::new("test.echo"));
    let mut h = Harness::new(pool, vec![gate.clone(), echo.clone()]);
    h.publish(
        "slow",
        json!([
            { "id": "wait", "action": "test.gate" },
            { "id": "after", "action": "test.echo" },
        ]),
    )
    .await;
    let run = h.start_run().await;
    let worker = h.worker("worker-a", config());
    gate.wait_for_a_call().await;

    let mut conn = h.pool.acquire().await.unwrap();
    assert!(cancel_run(&mut conn, run, h.clock.now()).await.unwrap());
    h.wait_for(run, RunStatus::Cancelled).await;
    worker.shut_down().await;

    assert!(gate.saw_cancellation());
    assert!(echo.calls().is_empty());
    let timeline = h.timeline(run).await;
    assert_eq!(ran(&timeline), [(Some("wait"), AttemptStatus::Cancelled)]);
    let cancelled = h.run(run).await;
    assert_eq!(cancelled.checkpoint.cursor, [json!("steps"), json!(0)]);
    assert!(cancelled.finished_at.is_some());
    assert!(h.jobs(run).await.is_empty());
    assert!(
        !cancel_run(&mut conn, run, h.clock.now()).await.unwrap(),
        "a finished Run cannot be cancelled"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_cancelled_sleeping_run_ends_without_waiting_for_its_wake_time(pool: PgPool) {
    let echo = Arc::new(FakeAction::new("test.echo"));
    let mut h = Harness::new(pool, vec![echo.clone()]);
    h.publish(
        "nap",
        json!([
            { "id": "nap", "action": "flow.sleep", "params": { "duration": "1d" } },
            { "id": "after", "action": "test.echo" },
        ]),
    )
    .await;
    let run = h.start_run().await;
    let worker = h.worker("worker-a", config());
    h.wait_for(run, RunStatus::Sleeping).await;

    let mut conn = h.pool.acquire().await.unwrap();
    assert!(cancel_run(&mut conn, run, h.clock.now()).await.unwrap());
    h.wait_for(run, RunStatus::Cancelled).await;
    worker.shut_down().await;

    assert!(echo.calls().is_empty());
    let timeline = h.timeline(run).await;
    assert_eq!(ran(&timeline), [(None, AttemptStatus::Cancelled)]);
    assert!(h.jobs(run).await.is_empty());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn heartbeats_keep_a_long_attempt_leased_and_notice_a_cancel_whose_notification_was_lost(
    pool: PgPool,
) {
    let gate = Gate::new();
    let mut h = Harness::new(pool, vec![gate.clone()]);
    h.publish("slow", json!([{ "id": "wait", "action": "test.gate" }]))
        .await;
    let run = h.start_run().await;
    let worker = h.worker("worker-a", config());
    gate.wait_for_a_call().await;

    for beat in 1..=4 {
        h.clock.advance(LEASE / 3);
        let until = t0() + LEASE / 3 * beat + LEASE;
        eventually("the lease to be extended", || async {
            h.jobs(run).await[0].locked_until == Some(until)
        })
        .await;
    }
    let mut conn = h.pool.acquire().await.unwrap();
    let taken = PgJobRepository
        .claim(&mut conn, "worker-b", 1, LEASE, h.clock.now())
        .await
        .unwrap();
    assert!(
        taken.is_empty(),
        "past the first Lease, the Job is still held"
    );

    sqlx::query("ALTER TABLE runs DISABLE TRIGGER runs_cancel_requested")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(cancel_run(&mut conn, run, h.clock.now()).await.unwrap());
    h.clock.advance(LEASE / 3);
    h.wait_for(run, RunStatus::Cancelled).await;
    worker.shut_down().await;

    assert!(gate.saw_cancellation());
}
