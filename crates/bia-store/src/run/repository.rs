use std::future::Future;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgConnection;
use sqlx::types::Json;

use super::row::RunRow;
use super::{Checkpoint, NewRun, Run, RunFilter, RunId, RunStatus};
use crate::StoreError;
use crate::version::why_not_published;

pub trait RunRepository {
    fn create(
        &self,
        conn: &mut PgConnection,
        run: &NewRun,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<Run, StoreError>> + Send;

    fn get(
        &self,
        conn: &mut PgConnection,
        id: RunId,
    ) -> impl Future<Output = Result<Option<Run>, StoreError>> + Send;

    /// Records a transition at `at`: the Run's new status and Checkpoint. A finished status
    /// also sets `finished_at`.
    fn update_checkpoint(
        &self,
        conn: &mut PgConnection,
        id: RunId,
        status: RunStatus,
        checkpoint: &Checkpoint,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    fn list(
        &self,
        conn: &mut PgConnection,
        filter: &RunFilter,
    ) -> impl Future<Output = Result<Vec<Run>, StoreError>> + Send;

    /// Flags the Run for cancellation at `at` and notifies Workers once the surrounding
    /// transaction commits. Returns `false`, changing nothing, when the Run already finished.
    fn request_cancel(
        &self,
        conn: &mut PgConnection,
        id: RunId,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgRunRepository;

impl RunRepository for PgRunRepository {
    async fn create(
        &self,
        conn: &mut PgConnection,
        run: &NewRun,
        at: DateTime<Utc>,
    ) -> Result<Run, StoreError> {
        let row = sqlx::query_file_as!(
            RunRow,
            "src/run/queries/insert.sql",
            run.workflow_id.as_str(),
            run.version.get(),
            run.input,
            run.trigger,
            Json(&run.checkpoint.cursor) as _,
            run.checkpoint.state,
            at,
        )
        .fetch_optional(&mut *conn)
        .await?;

        match row {
            Some(row) => row.try_into(),
            None => Err(why_not_published(conn, &run.workflow_id, run.version).await?),
        }
    }

    async fn get(&self, conn: &mut PgConnection, id: RunId) -> Result<Option<Run>, StoreError> {
        sqlx::query_file_as!(RunRow, "src/run/queries/get.sql", id.as_uuid())
            .fetch_optional(&mut *conn)
            .await?
            .map(Run::try_from)
            .transpose()
    }

    async fn update_checkpoint(
        &self,
        conn: &mut PgConnection,
        id: RunId,
        status: RunStatus,
        checkpoint: &Checkpoint,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let finished_at = status.is_finished().then_some(at);
        let updated = sqlx::query_file!(
            "src/run/queries/update_checkpoint.sql",
            id.as_uuid(),
            status.as_str(),
            Json(&checkpoint.cursor) as _,
            checkpoint.state,
            at,
            finished_at,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();

        if updated == 0 {
            return Err(StoreError::RunNotFound(id));
        }
        Ok(())
    }

    async fn list(
        &self,
        conn: &mut PgConnection,
        filter: &RunFilter,
    ) -> Result<Vec<Run>, StoreError> {
        sqlx::query_file_as!(
            RunRow,
            "src/run/queries/list.sql",
            filter.workflow_id.as_ref().map(|id| id.as_str()),
            filter.status.map(RunStatus::as_str),
            filter.after.map(RunId::as_uuid),
            i64::from(filter.limit),
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(Run::try_from)
        .collect()
    }

    async fn request_cancel(
        &self,
        conn: &mut PgConnection,
        id: RunId,
        at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let flagged =
            sqlx::query_file_scalar!("src/run/queries/request_cancel.sql", id.as_uuid(), at)
                .fetch_optional(&mut *conn)
                .await?
                .is_some();
        if !flagged && self.get(conn, id).await?.is_none() {
            return Err(StoreError::RunNotFound(id));
        }
        Ok(flagged)
    }
}
