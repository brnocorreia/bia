use std::future::Future;

use sqlx::PgConnection;

use crate::StoreError;
use crate::models::{Job, JobId, NewJob, RunId};
use crate::rows::JobRow;

/// Jobs: internal units of engine work. Enqueueing and completing them here lets a
/// transition do both in its transaction (ADR-0010); claims, Leases and retries build on the
/// same rows.
pub trait JobRepository {
    /// Enqueues a Job.
    fn enqueue(
        &self,
        conn: &mut PgConnection,
        job: &NewJob,
    ) -> impl Future<Output = Result<JobId, StoreError>> + Send;

    /// Completes a Job, removing it from the table.
    fn complete(
        &self,
        conn: &mut PgConnection,
        id: JobId,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Lists the pending Jobs of a Run, oldest first.
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
            "queries/jobs/insert.sql",
            job.kind.as_str(),
            job.run_id.map(RunId::as_uuid),
            job.payload,
            job.run_at,
        )
        .fetch_one(&mut *conn)
        .await?;

        Ok(JobId::new(id))
    }

    async fn complete(&self, conn: &mut PgConnection, id: JobId) -> Result<(), StoreError> {
        let deleted = sqlx::query_file!("queries/jobs/delete.sql", id.get())
            .execute(&mut *conn)
            .await?
            .rows_affected();

        if deleted == 0 {
            return Err(StoreError::JobNotFound(id));
        }
        Ok(())
    }

    async fn list_for_run(
        &self,
        conn: &mut PgConnection,
        run: RunId,
    ) -> Result<Vec<Job>, StoreError> {
        sqlx::query_file_as!(JobRow, "queries/jobs/list_for_run.sql", run.as_uuid())
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(Job::try_from)
            .collect()
    }
}
