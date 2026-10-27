//! Queue Triggers dispatch each Message to one Run at a time, however many schedulers dispatch
//! at once and however many Workflows consume the Queue, and a Run keeps its Messages past
//! their visibility timeout (ADR-0061).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::sync::Arc;

use bia_core::clock::{Clock, ManualClock};
use bia_core::dsl::{Definition, QueueName, WorkflowId};
use bia_engine::{Scheduler, SchedulerConfig};
use bia_store::queue::{PgQueueRepository, QueueRepository, QueueSettings};
use bia_store::run::{PgRunRepository, Run, RunFilter, RunRepository};
use bia_store::version::{PgVersionRepository, VersionRepository};
use bia_store::workflow::{PgWorkflowRepository, WorkflowRepository};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;

const MESSAGES: i64 = 500;
const SCHEDULERS: usize = 4;

fn t0() -> DateTime<Utc> {
    "2026-05-01T12:00:00Z".parse().unwrap()
}

/// Publishes and activates a Workflow with one queue Trigger on `queue`.
async fn consume(pool: &PgPool, clock: &ManualClock, workflow: &str, queue: &str, batch: u32) {
    let definition = Definition::from_value(json!({
        "schema_version": 1,
        "id": workflow,
        "triggers": [{ "id": "work", "type": "queue", "queue": queue, "batch_size": batch }],
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
        .publish(&mut conn, &definition.id, draft.number, clock.now())
        .await
        .unwrap();
    PgWorkflowRepository
        .set_active_version(&mut conn, &definition.id, draft.number)
        .await
        .unwrap();
}

async fn runs(pool: &PgPool, workflow: &str) -> Vec<Run> {
    let mut conn = pool.acquire().await.unwrap();
    PgRunRepository
        .list(
            &mut conn,
            &RunFilter {
                workflow_id: Some(WorkflowId::new(workflow).unwrap()),
                limit: 1_000,
                ..RunFilter::default()
            },
        )
        .await
        .unwrap()
}

fn message_ids(run: &Run) -> Vec<i64> {
    run.trigger["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["id"].as_i64().unwrap())
        .collect()
}

/// Ticks every scheduler at once and returns how many Runs they started together.
async fn tick_all(schedulers: &[Arc<Scheduler>]) -> usize {
    let ticks: Vec<_> = schedulers
        .iter()
        .map(|scheduler| {
            let scheduler = Arc::clone(scheduler);
            tokio::spawn(async move { scheduler.tick().await.unwrap().len() })
        })
        .collect();
    let mut started = 0;
    for tick in ticks {
        started += tick.await.unwrap();
    }
    started
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_message_is_never_in_two_runs_across_schedulers_consumers_and_its_visibility_timeout(
    pool: PgPool,
) {
    let clock = ManualClock::new(t0());
    let queue = QueueName::new("jobs").unwrap();
    let mut conn = pool.acquire().await.unwrap();
    PgQueueRepository
        .put(&mut conn, &queue, &QueueSettings::default(), clock.now())
        .await
        .unwrap();
    let bodies: Vec<Value> = (0..MESSAGES).map(|n| json!(n)).collect();
    let published = PgQueueRepository
        .publish(&mut conn, &queue, &bodies, clock.now(), clock.now())
        .await
        .unwrap();
    drop(conn);
    consume(&pool, &clock, "billing", "jobs", 7).await;
    consume(&pool, &clock, "audit", "jobs", 9).await;
    let schedulers: Vec<Arc<Scheduler>> = (0..SCHEDULERS)
        .map(|i| {
            Arc::new(Scheduler::new(
                format!("scheduler-{i}"),
                pool.clone(),
                Arc::new(clock.clone()),
                SchedulerConfig::default(),
            ))
        })
        .collect();

    let started = tick_all(&schedulers).await;

    let all: Vec<Run> = [runs(&pool, "billing").await, runs(&pool, "audit").await].concat();
    assert_eq!(started, all.len());
    let mut seen = BTreeSet::new();
    for run in &all {
        assert_eq!(
            run.input["messages"].as_array().unwrap().len(),
            message_ids(run).len()
        );
        for id in message_ids(run) {
            assert!(seen.insert(id), "message {id} is in two Runs");
        }
    }
    let expected: BTreeSet<i64> = published.iter().map(|id| id.get()).collect();
    assert_eq!(seen, expected);

    // No Worker runs them, so every Run is still active long after the visibility timeout.
    clock.advance(TimeDelta::hours(1));
    assert_eq!(tick_all(&schedulers).await, 0);
}
