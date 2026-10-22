//! A transition's Checkpoint, Timeline entry and Jobs commit or roll back together (ADR-0010).

// `allow-unwrap-in-tests` covers `#[test]` functions only, not the helpers they share.
#![allow(clippy::unwrap_used)]

use bia_core::dsl::{ActionName, StepId};
use bia_store::StoreError;
use bia_store::job::{Claimed, JobRepository, Lease, NewJob, PgJobRepository};
use bia_store::run::{Checkpoint, PgRunRepository, RunId, RunRepository, RunStatus};
use bia_store::timeline::{
    AttemptStatus, NewTimelineEntry, PgTimelineRepository, StepAttempt, TimelineRepository,
};
use chrono::TimeDelta;
use serde_json::json;
use sqlx::{Acquire, PgConnection, PgPool};

mod common;

use common::{at, published_workflow, start_run};

async fn claim_one(conn: &mut PgConnection) -> Claimed {
    let mut claimed = PgJobRepository
        .claim(conn, "worker-a", 1, TimeDelta::seconds(30), at(1))
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    claimed.remove(0)
}

fn attempt(step: &str, attempt: i32) -> NewTimelineEntry {
    NewTimelineEntry {
        step: Some(StepAttempt {
            step_id: StepId::new(step).unwrap(),
            iteration_path: String::new(),
            attempt,
            action: ActionName::new("http.request").unwrap(),
        }),
        status: AttemptStatus::Succeeded,
        input: Some(json!({ "url": "https://example.com" })),
        output: Some(json!({ "status": 200 })),
        error: None,
        started_at: at(10),
        finished_at: at(12),
    }
}

fn after_first_step() -> Checkpoint {
    Checkpoint {
        cursor: vec![json!("steps"), json!(1)],
        state: json!({ "outputs": { "fetch": { "status": 200 } } }),
    }
}

/// What a Worker writes when a Step finishes: Timeline entry, Checkpoint, Job completion and
/// the next Job.
async fn transition(conn: &mut PgConnection, run: RunId, lease: &Lease) {
    PgTimelineRepository
        .append(conn, run, &attempt("fetch", 1))
        .await
        .unwrap();
    PgRunRepository
        .update_checkpoint(conn, run, RunStatus::Running, &after_first_step(), at(12))
        .await
        .unwrap();
    PgJobRepository.ack(conn, lease).await.unwrap();
    PgJobRepository
        .enqueue(conn, &NewJob::advance_run(run, at(12)))
        .await
        .unwrap();
}

#[sqlx::test]
async fn a_committed_transition_is_visible(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let version = published_workflow(&mut conn, "onboarding-email").await;
    let run = start_run(&mut conn, &version, at(0)).await;
    let first_job = PgJobRepository
        .enqueue(&mut conn, &NewJob::advance_run(run.id, at(0)))
        .await
        .unwrap();

    let claimed = claim_one(&mut conn).await;

    let mut tx = pool.begin().await.unwrap();
    transition(&mut tx, run.id, &claimed.lease).await;
    tx.commit().await.unwrap();

    let stored = PgRunRepository
        .get(&mut conn, run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, RunStatus::Running);
    assert_eq!(stored.checkpoint, after_first_step());
    let entries = PgTimelineRepository.list(&mut conn, run.id).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, 1);
    assert_eq!(entries[0].output, Some(json!({ "status": 200 })));
    assert_eq!(entries[0].duration().num_seconds(), 2);
    let pending = PgJobRepository
        .list_for_run(&mut conn, run.id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_ne!(pending[0].id, first_job);
}

#[sqlx::test]
async fn a_rolled_back_transition_leaves_nothing(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let version = published_workflow(&mut conn, "onboarding-email").await;
    let run = start_run(&mut conn, &version, at(0)).await;
    let first_job = PgJobRepository
        .enqueue(&mut conn, &NewJob::advance_run(run.id, at(0)))
        .await
        .unwrap();

    let claimed = claim_one(&mut conn).await;

    let mut tx = pool.begin().await.unwrap();
    transition(&mut tx, run.id, &claimed.lease).await;
    tx.rollback().await.unwrap();

    let stored = PgRunRepository
        .get(&mut conn, run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored, run);
    assert!(
        PgTimelineRepository
            .list(&mut conn, run.id)
            .await
            .unwrap()
            .is_empty()
    );
    let pending = PgJobRepository
        .list_for_run(&mut conn, run.id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, first_job);

    // The rolled-back entry did not use up a place in the Timeline.
    assert_eq!(
        PgTimelineRepository
            .append(&mut conn, run.id, &attempt("fetch", 1))
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test]
async fn a_failed_repository_call_inside_a_transaction_keeps_the_transaction_usable(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let version = published_workflow(&mut conn, "onboarding-email").await;
    let run = start_run(&mut conn, &version, at(0)).await;

    let mut tx = pool.begin().await.unwrap();
    let missing = RunId::from_uuid(uuid::Uuid::nil());
    let failed = PgTimelineRepository
        .append(&mut tx, missing, &attempt("fetch", 1))
        .await;
    assert!(
        matches!(failed, Err(StoreError::RunNotFound(_))),
        "{failed:?}"
    );
    PgTimelineRepository
        .append(&mut tx, run.id, &attempt("fetch", 1))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        PgTimelineRepository
            .list(&mut conn, run.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[sqlx::test]
async fn timeline_entries_keep_their_order(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let version = published_workflow(&mut conn, "onboarding-email").await;
    let run = start_run(&mut conn, &version, at(0)).await;

    let mut failed = attempt("fetch", 1);
    failed.status = AttemptStatus::Failed;
    failed.output = None;
    failed.error = Some(json!({ "class": "retryable", "message": "timeout" }));
    let mut skipped = attempt("notify", 1);
    skipped.status = AttemptStatus::Skipped;
    skipped.input = None;
    skipped.output = None;
    let about_the_run = NewTimelineEntry {
        step: None,
        status: AttemptStatus::Failed,
        input: Some(json!({ "user": 42 })),
        output: None,
        error: Some(json!({ "class": "fatal", "message": "input rejected" })),
        started_at: at(13),
        finished_at: at(13),
    };

    for entry in [&failed, &attempt("fetch", 2), &skipped, &about_the_run] {
        let mut tx = conn.begin().await.unwrap();
        PgTimelineRepository
            .append(&mut tx, run.id, entry)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    let entries = PgTimelineRepository.list(&mut conn, run.id).await.unwrap();
    let summary: Vec<_> = entries
        .iter()
        .map(|entry| {
            (
                entry.seq,
                entry.step.as_ref().map(|step| step.step_id.as_str()),
                entry.step.as_ref().map(|step| step.attempt),
                entry.status,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (1, Some("fetch"), Some(1), AttemptStatus::Failed),
            (2, Some("fetch"), Some(2), AttemptStatus::Succeeded),
            (3, Some("notify"), Some(1), AttemptStatus::Skipped),
            (4, None, None, AttemptStatus::Failed),
        ]
    );
    assert_eq!(entries[0].error, failed.error);
    assert_eq!(entries[3].error, about_the_run.error);
}
