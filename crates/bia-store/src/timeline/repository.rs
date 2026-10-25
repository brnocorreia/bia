use std::future::Future;

use sqlx::{Acquire, PgConnection};

use super::row::TimelineRow;
use super::{NewTimelineEntry, TimelineEntry};
use crate::StoreError;
use crate::run::RunId;

pub trait TimelineRepository {
    /// Appends an entry to the end of a Run's Timeline and returns its `seq`.
    fn append(
        &self,
        conn: &mut PgConnection,
        run: RunId,
        entry: &NewTimelineEntry,
    ) -> impl Future<Output = Result<i32, StoreError>> + Send;

    fn list(
        &self,
        conn: &mut PgConnection,
        run: RunId,
    ) -> impl Future<Output = Result<Vec<TimelineEntry>, StoreError>> + Send;

    /// The last `failed` entry of each of `runs` that has one, in no particular order.
    fn last_failures(
        &self,
        conn: &mut PgConnection,
        runs: &[RunId],
    ) -> impl Future<Output = Result<Vec<TimelineEntry>, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgTimelineRepository;

impl TimelineRepository for PgTimelineRepository {
    async fn append(
        &self,
        conn: &mut PgConnection,
        run: RunId,
        entry: &NewTimelineEntry,
    ) -> Result<i32, StoreError> {
        let step = entry.step.as_ref();
        let mut tx = conn.begin().await?;

        let seq = sqlx::query_file_scalar!("src/timeline/queries/next_seq.sql", run.as_uuid())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StoreError::RunNotFound(run))?;

        sqlx::query_file!(
            "src/timeline/queries/insert.sql",
            run.as_uuid(),
            seq,
            step.map(|step| step.step_id.as_str()),
            step.map_or("", |step| step.iteration_path.as_str()),
            step.map(|step| step.attempt),
            step.map(|step| step.action.as_str()),
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
        sqlx::query_file_as!(TimelineRow, "src/timeline/queries/list.sql", run.as_uuid())
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(TimelineEntry::try_from)
            .collect()
    }

    async fn last_failures(
        &self,
        conn: &mut PgConnection,
        runs: &[RunId],
    ) -> Result<Vec<TimelineEntry>, StoreError> {
        let ids: Vec<_> = runs.iter().map(|run| run.as_uuid()).collect();
        sqlx::query_file_as!(TimelineRow, "src/timeline/queries/last_failures.sql", &ids)
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(TimelineEntry::try_from)
            .collect()
    }
}
