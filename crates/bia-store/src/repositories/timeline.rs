use std::future::Future;

use sqlx::{Acquire, PgConnection};

use crate::StoreError;
use crate::models::{NewTimelineEntry, RunId, TimelineEntry};
use crate::rows::TimelineRow;

/// Timelines: the ordered record of a Run, one entry per Attempt.
pub trait TimelineRepository {
    /// Appends an Attempt to the end of a Run's Timeline and returns its `seq`.
    fn append(
        &self,
        conn: &mut PgConnection,
        run: RunId,
        entry: &NewTimelineEntry,
    ) -> impl Future<Output = Result<i32, StoreError>> + Send;

    /// Lists a Run's Timeline in order.
    fn list(
        &self,
        conn: &mut PgConnection,
        run: RunId,
    ) -> impl Future<Output = Result<Vec<TimelineEntry>, StoreError>> + Send;
}

/// [`TimelineRepository`] over Postgres.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgTimelineRepository;

impl TimelineRepository for PgTimelineRepository {
    async fn append(
        &self,
        conn: &mut PgConnection,
        run: RunId,
        entry: &NewTimelineEntry,
    ) -> Result<i32, StoreError> {
        let mut tx = conn.begin().await?;

        let seq = sqlx::query_file_scalar!("queries/timeline/next_seq.sql", run.as_uuid())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StoreError::RunNotFound(run))?;

        sqlx::query_file!(
            "queries/timeline/insert.sql",
            run.as_uuid(),
            seq,
            entry.step_id.as_str(),
            entry.iteration_path,
            entry.attempt,
            entry.action.as_str(),
            entry.status.as_str(),
            entry.input,
            entry.output,
            entry.error,
            entry.started_at,
            entry.finished_at,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(seq)
    }

    async fn list(
        &self,
        conn: &mut PgConnection,
        run: RunId,
    ) -> Result<Vec<TimelineEntry>, StoreError> {
        sqlx::query_file_as!(TimelineRow, "queries/timeline/list.sql", run.as_uuid())
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(TimelineEntry::try_from)
            .collect()
    }
}
