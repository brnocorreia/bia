//! Job claims, Leases, retries and wake-ups, with time driven by the manual Clock.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bia_core::clock::{Clock, ManualClock};
use bia_store::StoreError;
use bia_store::job::{
    Backoff, Claimed, Failed, JobId, JobKind, JobListener, JobRepository, NewJob, PgJobRepository,
    Wakeup,
};
use bia_store::run::RunId;
use chrono::TimeDelta;
use sqlx::{PgConnection, PgPool};

mod common;

use common::{published_workflow, start_run, t0};

const LEASE: TimeDelta = TimeDelta::seconds(30);

async fn a_run(conn: &mut PgConnection) -> RunId {
    let version = published_workflow(conn, "onboarding-email").await;
    start_run(conn, &version, t0()).await.id
}

async fn claim(conn: &mut PgConnection, worker: &str, clock: &ManualClock) -> Vec<Claimed> {
    PgJobRepository
        .claim(conn, worker, 10, LEASE, clock.now())
        .await
        .unwrap()
}

#[sqlx::test]
async fn many_concurrent_claimers_process_each_job_exactly_once(pool: PgPool) {
    const JOBS: usize = 300;
    const CLAIMERS: usize = 16;

    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    let version = published_workflow(&mut conn, "onboarding-email").await;
    let mut enqueued = Vec::with_capacity(JOBS);
    let mut run = None;
    for i in 0..JOBS {
        if i % 10 == 0 {
            run = Some(start_run(&mut conn, &version, t0()).await.id);
        }
        let run = run.unwrap();
        enqueued.push(
            PgJobRepository
                .enqueue(&mut conn, &NewJob::advance_run(run, clock.now()))
                .await
                .unwrap(),
        );
    }
    drop(conn);

    let processed: Arc<Mutex<Vec<(JobId, String, i32)>>> = Arc::default();
    let mut claimers = Vec::new();
    for c in 0..CLAIMERS {
        let pool = pool.clone();
        let clock = clock.clone();
        let processed = Arc::clone(&processed);
        claimers.push(tokio::spawn(async move {
            let worker = format!("worker-{c}");
            loop {
                let mut conn = pool.acquire().await.unwrap();
                let batch = PgJobRepository
                    .claim(&mut conn, &worker, 7, TimeDelta::minutes(5), clock.now())
                    .await
                    .unwrap();
                drop(conn);
                if batch.is_empty() {
                    break;
                }
                for claimed in batch {
                    tokio::task::yield_now().await;
                    let mut tx = pool.begin().await.unwrap();
                    PgJobRepository.ack(&mut tx, &claimed.lease).await.unwrap();
                    tx.commit().await.unwrap();
                    processed.lock().unwrap().push((
                        claimed.job.id,
                        worker.clone(),
                        claimed.lease.attempt(),
                    ));
                }
            }
        }));
    }
    for claimer in claimers {
        claimer.await.unwrap();
    }

    let processed = std::mem::take(&mut *processed.lock().unwrap());
    let mut times_processed: BTreeMap<JobId, usize> = BTreeMap::new();
    for (id, _, attempt) in processed.iter() {
        assert_eq!(*attempt, 1, "job {id} was claimed after a Lease expiry");
        *times_processed.entry(*id).or_default() += 1;
    }
    enqueued.sort();
    assert_eq!(
        times_processed.keys().copied().collect::<Vec<_>>(),
        enqueued
    );
    assert!(times_processed.values().all(|times| *times == 1));
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[sqlx::test]
async fn an_expired_lease_lets_another_claimer_take_the_job(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    let run = a_run(&mut conn).await;
    let id = PgJobRepository
        .enqueue(&mut conn, &NewJob::advance_run(run, clock.now()))
        .await
        .unwrap();

    let first = claim(&mut conn, "worker-a", &clock).await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].job.id, id);
    assert_eq!(first[0].lease.until(), t0() + LEASE);
    assert_eq!(first[0].job.locked_by.as_deref(), Some("worker-a"));

    clock.advance(LEASE - TimeDelta::seconds(1));
    assert!(claim(&mut conn, "worker-b", &clock).await.is_empty());

    clock.advance(TimeDelta::seconds(1));
    let second = claim(&mut conn, "worker-b", &clock).await;
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].job.id, id);
    assert_eq!(second[0].lease.attempt(), 2);
    assert_eq!(second[0].job.locked_by.as_deref(), Some("worker-b"));

    // The first holder's late calls are fenced off; the Job stays with worker-b.
    let late = PgJobRepository.ack(&mut conn, &first[0].lease).await;
    assert!(
        matches!(late, Err(StoreError::LeaseLost(job)) if job == id),
        "{late:?}"
    );
    let late = PgJobRepository
        .fail(
            &mut conn,
            &first[0].lease,
            "boom",
            &Backoff::default(),
            clock.now(),
        )
        .await;
    assert!(matches!(late, Err(StoreError::LeaseLost(_))), "{late:?}");
    let late = PgJobRepository
        .extend_lease(&mut conn, &first[0].lease, LEASE, clock.now())
        .await;
    assert!(matches!(late, Err(StoreError::LeaseLost(_))), "{late:?}");
    let job = PgJobRepository.get(&mut conn, id).await.unwrap().unwrap();
    assert_eq!(job.locked_by.as_deref(), Some("worker-b"));
    assert_eq!(job.last_error, None);

    PgJobRepository
        .ack(&mut conn, &second[0].lease)
        .await
        .unwrap();
    assert_eq!(PgJobRepository.get(&mut conn, id).await.unwrap(), None);
    let again = PgJobRepository.ack(&mut conn, &second[0].lease).await;
    assert!(matches!(again, Err(StoreError::LeaseLost(_))), "{again:?}");
}

#[sqlx::test]
async fn an_extended_lease_keeps_the_job(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    let run = a_run(&mut conn).await;
    PgJobRepository
        .enqueue(&mut conn, &NewJob::advance_run(run, clock.now()))
        .await
        .unwrap();
    let claimed = claim(&mut conn, "worker-a", &clock).await.remove(0);

    clock.advance(LEASE - TimeDelta::seconds(5));
    let extended = PgJobRepository
        .extend_lease(&mut conn, &claimed.lease, LEASE, clock.now())
        .await
        .unwrap();
    assert_eq!(extended.until(), clock.now() + LEASE);
    assert_eq!(extended.attempt(), claimed.lease.attempt());

    clock.advance(LEASE - TimeDelta::seconds(1));
    assert!(claim(&mut conn, "worker-b", &clock).await.is_empty());
    PgJobRepository.ack(&mut conn, &extended).await.unwrap();
}

#[sqlx::test]
async fn a_released_job_is_claimable_at_once_and_its_old_lease_is_fenced(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut listener = JobListener::connect(&pool).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let run = a_run(&mut conn).await;
    PgJobRepository
        .enqueue(&mut conn, &NewJob::advance_run(run, clock.now()))
        .await
        .unwrap();
    listener.recv().await.unwrap();
    let first = claim(&mut conn, "worker-a", &clock).await.remove(0);

    PgJobRepository
        .release(&mut conn, &first.lease)
        .await
        .unwrap();

    let woken = tokio::time::timeout(Duration::from_secs(10), listener.recv())
        .await
        .expect("a release wakes listeners")
        .unwrap();
    assert_eq!(woken, Wakeup::JobClaimable(JobKind::AdvanceRun));
    let second = claim(&mut conn, "worker-b", &clock).await.remove(0);
    assert_eq!(
        second.job.id, first.job.id,
        "no wait for the Lease to expire"
    );
    assert_eq!(second.lease.attempt(), 2, "the released Attempt counts");
    let late = PgJobRepository.ack(&mut conn, &first.lease).await;
    assert!(matches!(late, Err(StoreError::LeaseLost(_))), "{late:?}");
    let again = PgJobRepository.release(&mut conn, &first.lease).await;
    assert!(matches!(again, Err(StoreError::LeaseLost(_))), "{again:?}");
}

#[sqlx::test]
async fn bringing_a_run_forward_moves_only_its_waiting_jobs(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    let version = published_workflow(&mut conn, "onboarding-email").await;
    let run = start_run(&mut conn, &version, t0()).await.id;
    let other = start_run(&mut conn, &version, t0()).await.id;
    let later = t0() + TimeDelta::days(3);
    let waiting = PgJobRepository
        .enqueue(&mut conn, &NewJob::advance_run(run, later))
        .await
        .unwrap();
    let elsewhere = PgJobRepository
        .enqueue(&mut conn, &NewJob::advance_run(other, later))
        .await
        .unwrap();

    let moved = PgJobRepository
        .bring_forward(&mut conn, run, clock.now())
        .await
        .unwrap();

    assert_eq!(moved, 1);
    let claimed = claim(&mut conn, "worker-a", &clock).await;
    let claimed: Vec<_> = claimed.iter().map(|claimed| claimed.job.id).collect();
    assert_eq!(claimed, [waiting]);
    let untouched = PgJobRepository.get(&mut conn, elsewhere).await.unwrap();
    assert_eq!(untouched.unwrap().run_at, later);
    let moved = PgJobRepository
        .bring_forward(&mut conn, run, clock.now())
        .await
        .unwrap();
    assert_eq!(moved, 0, "a Job under a Lease stays as it is");
}

#[sqlx::test]
async fn a_delayed_job_is_claimable_from_its_run_at(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    let run = a_run(&mut conn).await;
    let wake_at = t0() + TimeDelta::days(3);
    let id = PgJobRepository
        .enqueue(&mut conn, &NewJob::advance_run(run, wake_at))
        .await
        .unwrap();

    assert!(claim(&mut conn, "worker-a", &clock).await.is_empty());
    clock.set(wake_at - TimeDelta::milliseconds(1));
    assert!(claim(&mut conn, "worker-a", &clock).await.is_empty());

    clock.set(wake_at);
    let claimed = claim(&mut conn, "worker-a", &clock).await;
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].job.id, id);
}

#[sqlx::test]
async fn due_jobs_are_claimed_oldest_first_up_to_the_limit(pool: PgPool) {
    let clock = ManualClock::new(t0() + TimeDelta::minutes(1));
    let mut conn = pool.acquire().await.unwrap();
    let run = a_run(&mut conn).await;
    let mut ids = Vec::new();
    for seconds in [30, 10, 20] {
        let job = NewJob::advance_run(run, t0() + TimeDelta::seconds(seconds));
        ids.push(PgJobRepository.enqueue(&mut conn, &job).await.unwrap());
    }

    let claimed = PgJobRepository
        .claim(&mut conn, "worker-a", 2, LEASE, clock.now())
        .await
        .unwrap();
    let claimed: Vec<_> = claimed.iter().map(|claimed| claimed.job.id).collect();
    assert_eq!(claimed, [ids[1], ids[2]]);
}

#[sqlx::test]
async fn a_failed_job_retries_with_backoff_then_gives_up(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    let run = a_run(&mut conn).await;
    let mut job = NewJob::advance_run(run, clock.now());
    job.max_attempts = 3;
    let id = PgJobRepository.enqueue(&mut conn, &job).await.unwrap();
    let backoff = Backoff {
        initial: TimeDelta::seconds(10),
        max: TimeDelta::minutes(5),
    };

    for (attempt, delay) in [(1, 10), (2, 20)] {
        let claimed = claim(&mut conn, "worker-a", &clock).await.remove(0);
        assert_eq!(claimed.lease.attempt(), attempt);
        let failed = PgJobRepository
            .fail(&mut conn, &claimed.lease, "timeout", &backoff, clock.now())
            .await
            .unwrap();
        let retry_at = clock.now() + TimeDelta::seconds(delay);
        assert_eq!(failed, Failed::Retrying { run_at: retry_at });

        clock.set(retry_at - TimeDelta::seconds(1));
        assert!(claim(&mut conn, "worker-a", &clock).await.is_empty());
        clock.set(retry_at);
    }

    let last = claim(&mut conn, "worker-a", &clock).await.remove(0);
    assert_eq!(last.lease.attempt(), 3);
    let failed = PgJobRepository
        .fail(
            &mut conn,
            &last.lease,
            "still timing out",
            &backoff,
            clock.now(),
        )
        .await
        .unwrap();
    assert_eq!(failed, Failed::GaveUp);

    let job = PgJobRepository.get(&mut conn, id).await.unwrap().unwrap();
    assert_eq!(job.failed_at, Some(clock.now()));
    assert_eq!(job.attempts, 3);
    assert_eq!(job.last_error.as_deref(), Some("still timing out"));
    assert_eq!(job.locked_until, None);
    clock.advance(TimeDelta::days(30));
    assert!(claim(&mut conn, "worker-a", &clock).await.is_empty());
    let late = PgJobRepository.ack(&mut conn, &last.lease).await;
    assert!(matches!(late, Err(StoreError::LeaseLost(_))), "{late:?}");
}

#[test]
fn backoff_doubles_up_to_its_cap() {
    let backoff = Backoff::default();
    let delays: Vec<_> = [1, 2, 3, 12, 13, 1_000]
        .map(|attempt| backoff.delay(attempt).num_seconds())
        .into();
    assert_eq!(delays, [1, 2, 4, 2048, 3600, 3600]);
}

#[sqlx::test]
async fn a_committed_enqueue_wakes_a_listener(pool: PgPool) {
    let mut listener = JobListener::connect(&pool).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let run = a_run(&mut conn).await;

    let mut tx = pool.begin().await.unwrap();
    PgJobRepository
        .enqueue(&mut tx, &NewJob::advance_run(run, t0()))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let woken = tokio::time::timeout(Duration::from_secs(10), listener.recv())
        .await
        .expect("no notification arrived")
        .unwrap();
    assert_eq!(woken, Wakeup::JobClaimable(JobKind::AdvanceRun));
}

/// Every behavioural timestamp comes from the Clock (ADR-0022), so no compile-checked query
/// reads the database's time.
#[test]
fn no_query_reads_the_database_clock() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.sqlx");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        let query: serde_json::Value = serde_json::from_str(&text).unwrap();
        let sql = query["query"].as_str().unwrap().to_lowercase();
        for reads_time in [
            "now()",
            "current_timestamp",
            "clock_timestamp",
            "localtimestamp",
        ] {
            assert!(
                !sql.contains(reads_time),
                "query reads `{reads_time}`: {sql}"
            );
        }
        checked += 1;
    }
    assert!(checked > 0);
}
