use std::future::Future;

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::PgConnection;

use super::row::JobRow;
use super::{Backoff, Claimed, Failed, Job, JobId, Lease, NewJob};
use crate::StoreError;
use crate::run::RunId;

/// Every method takes a connection, so a transition can ack its Job and enqueue the next one
/// in its own transaction (ADR-0010), and takes `now` from the caller's `Clock` (ADR-0022).
/// Claims skip Jobs another claimer is locking, so any number of Workers can claim without
/// taking the same Job twice. A Lease is fenced by the Job's attempt counter: once a Job is
/// claimed again after its Lease expired, the earlier holder can no longer ack, fail or extend
/// it. Semantics: ADR-0031.
pub trait JobRepository {
    /// Enqueues a Job. Listeners are notified when the surrounding transaction commits.
    fn enqueue(
        &self,
        conn: &mut PgConnection,
        job: &NewJob,
    ) -> impl Future<Output = Result<JobId, StoreError>> + Send;

    /// Claims up to `limit` Jobs that are due at `now` for `worker`, each under a Lease of
    /// `lease_for`. Jobs whose Lease expired are claimable again. Due Jobs come oldest
    /// `run_at` first; Jobs another claimer is locking right now are skipped rather than
    /// waited for.
    fn claim(
        &self,
        conn: &mut PgConnection,
        worker: &str,
        limit: i64,
        lease_for: TimeDelta,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<Claimed>, StoreError>> + Send;

    /// Extends `lease` to `now + lease_for`, so a long Attempt keeps its Job.
    fn extend_lease(
        &self,
        conn: &mut PgConnection,
        lease: &Lease,
        lease_for: TimeDelta,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Lease, StoreError>> + Send;

    /// Completes the Job `lease` is on, removing it.
    ///
    /// Fails with [`StoreError::LeaseLost`] once another claimer has taken the Job, so a
    /// holder whose Lease expired cannot complete it twice. A Lease that expired without
    /// anyone claiming the Job since still acks.
    fn ack(
        &self,
        conn: &mut PgConnection,
        lease: &Lease,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Records that the Attempt under `lease` failed with `error`. The Job is retried after
    /// `backoff`, or gives up once it has used `max_attempts` Attempts.
    fn fail(
        &self,
        conn: &mut PgConnection,
        lease: &Lease,
        error: &str,
        backoff: &Backoff,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Failed, StoreError>> + Send;

    /// Reads a Job, including one that gave up.
    fn get(
        &self,
        conn: &mut PgConnection,
        id: JobId,
    ) -> impl Future<Output = Result<Option<Job>, StoreError>> + Send;

    /// Lists the Jobs of a Run that have not completed, oldest first.
    fn list_for_run(
        &self,
        conn: &mut PgConnection,
        run: RunId,
    ) -> impl Future<Output = Result<Vec<Job>, StoreError>> + Send;
}

/// [`JobRepository`] over Postgres.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgJobRepository;

impl JobRepository for PgJobRepository {
    async fn enqueue(&self, conn: &mut PgConnection, job: &NewJob) -> Result<JobId, StoreError> {
        let id = sqlx::query_file_scalar!(
            "src/job/queries/insert.sql",
            job.kind.as_str(),
            job.run_id.map(RunId::as_uuid),
            job.payload,
            job.run_at,
            job.max_attempts,
        )
        .fetch_one(&mut *conn)
        .await?;

        Ok(JobId::new(id))
    }

    async fn claim(
        &self,
        conn: &mut PgConnection,
        worker: &str,
        limit: i64,
        lease_for: TimeDelta,
        now: DateTime<Utc>,
    ) -> Result<Vec<Claimed>, StoreError> {
        let until = now + lease_for;
        let mut claimed = sqlx::query_file_as!(
            JobRow,
            "src/job/queries/claim.sql",
            worker,
            until,
            now,
            limit,
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|row| {
            let job = Job::try_from(row)?;
            let lease = Lease::new(job.id, job.attempts, until);
            Ok(Claimed { job, lease })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;

        // `RETURNING` has no order of its own.
        claimed.sort_by_key(|claimed| (claimed.job.run_at, claimed.job.id));
        Ok(claimed)
    }

    async fn extend_lease(
        &self,
        conn: &mut PgConnection,
        lease: &Lease,
        lease_for: TimeDelta,
        now: DateTime<Utc>,
    ) -> Result<Lease, StoreError> {
        let until = now + lease_for;
        let extended = sqlx::query_file!(
            "src/job/queries/extend_lease.sql",
            lease.job_id().get(),
            lease.attempt(),
            until,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();

        if extended == 0 {
            return Err(StoreError::LeaseLost(lease.job_id()));
        }
        Ok(Lease::new(lease.job_id(), lease.attempt(), until))
    }

    async fn ack(&self, conn: &mut PgConnection, lease: &Lease) -> Result<(), StoreError> {
        let deleted = sqlx::query_file!(
            "src/job/queries/ack.sql",
            lease.job_id().get(),
            lease.attempt(),
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();

        if deleted == 0 {
            return Err(StoreError::LeaseLost(lease.job_id()));
        }
        Ok(())
    }

    async fn fail(
        &self,
        conn: &mut PgConnection,
        lease: &Lease,
        error: &str,
        backoff: &Backoff,
        now: DateTime<Utc>,
    ) -> Result<Failed, StoreError> {
        let retry_at = now + backoff.delay(lease.attempt());
        let row = sqlx::query_file!(
            "src/job/queries/fail.sql",
            lease.job_id().get(),
            lease.attempt(),
            error,
            retry_at,
            now,
        )
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(StoreError::LeaseLost(lease.job_id()))?;

        Ok(match row.failed_at {
            Some(_) => Failed::GaveUp,
            None => Failed::Retrying { run_at: row.run_at },
        })
    }

    async fn get(&self, conn: &mut PgConnection, id: JobId) -> Result<Option<Job>, StoreError> {
        sqlx::query_file_as!(JobRow, "src/job/queries/get.sql", id.get())
            .fetch_optional(&mut *conn)
            .await?
            .map(Job::try_from)
            .transpose()
    }

    async fn list_for_run(
        &self,
        conn: &mut PgConnection,
        run: RunId,
    ) -> Result<Vec<Job>, StoreError> {
        sqlx::query_file_as!(JobRow, "src/job/queries/list_for_run.sql", run.as_uuid())
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(Job::try_from)
            .collect()
    }
}
