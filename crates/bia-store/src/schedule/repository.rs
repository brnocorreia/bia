use std::future::Future;

use bia_core::dsl::WorkflowId;
use chrono::{DateTime, Utc};
use sqlx::{Acquire, PgConnection};

use super::row::{ConsumerRow, ScheduleRow};
use super::{DueSchedule, QueueConsumer, ReadyConsumer, Schedule, SyncState};
use crate::StoreError;
use crate::run::RunId;
use crate::version::{VersionNumber, version_number};
use crate::workflow::workflow_id;

/// Schedules and [`QueueConsumer`]s are rebuilt by the scheduler whenever a Workflow's Active
/// Version changes. Schedules fire under a row lock, so any number of schedulers can call
/// these methods at once without firing an occurrence twice (ADR-0039); queue Triggers
/// dispatch under the Messages' row locks (ADR-0061).
pub trait ScheduleRepository {
    /// Workflows whose Schedules were not built from their current Active Version, including
    /// Workflows that have none any more.
    fn out_of_sync(
        &self,
        conn: &mut PgConnection,
    ) -> impl Future<Output = Result<Vec<WorkflowId>, StoreError>> + Send;

    /// Locks the Workflow until the surrounding transaction ends, so its Active Version stays
    /// put while its Schedules are rebuilt. `None` when the Workflow does not exist.
    fn lock_for_sync(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
    ) -> impl Future<Output = Result<Option<SyncState>, StoreError>> + Send;

    fn list_for_workflow(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
    ) -> impl Future<Output = Result<Vec<Schedule>, StoreError>> + Send;

    /// Makes `schedules` and `consumers` the Workflow's only ones and records that they were
    /// built from `version`, its Active Version.
    fn replace(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        version: Option<VersionNumber>,
        schedules: &[Schedule],
        consumers: &[QueueConsumer],
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The [`QueueConsumer`]s of Active Versions whose Queue has a Message claimable at `now`.
    fn ready_consumers(
        &self,
        conn: &mut PgConnection,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<ReadyConsumer>, StoreError>> + Send;

    /// Whether `version` is the Workflow's Active Version. When it is, it stays so until the
    /// surrounding transaction ends.
    fn lock_active(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        version: VersionNumber,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Claims one Schedule with something to fire at `now`: an occurrence that fell due, or a
    /// queued one whose previous Run finished. The claim lasts until the surrounding
    /// transaction ends, and other claimers skip it meanwhile. Schedules of a Version that is
    /// no longer active are never claimed.
    fn claim_due(
        &self,
        conn: &mut PgConnection,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<DueSchedule>, StoreError>> + Send;

    /// Writes a Schedule back after firing it.
    fn save(
        &self,
        conn: &mut PgConnection,
        schedule: &Schedule,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    fn delete(
        &self,
        conn: &mut PgConnection,
        schedule: &Schedule,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgScheduleRepository;

impl ScheduleRepository for PgScheduleRepository {
    async fn out_of_sync(&self, conn: &mut PgConnection) -> Result<Vec<WorkflowId>, StoreError> {
        sqlx::query_file_scalar!("src/schedule/queries/out_of_sync.sql")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(workflow_id)
            .collect()
    }

    async fn lock_for_sync(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
    ) -> Result<Option<SyncState>, StoreError> {
        let Some(row) =
            sqlx::query_file!("src/schedule/queries/lock_for_sync.sql", workflow.as_str())
                .fetch_optional(&mut *conn)
                .await?
        else {
            return Ok(None);
        };
        Ok(Some(SyncState {
            active: row.active_version.map(version_number).transpose()?,
            scheduled: row.scheduled_version.map(version_number).transpose()?,
        }))
    }

    async fn list_for_workflow(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
    ) -> Result<Vec<Schedule>, StoreError> {
        sqlx::query_file_as!(
            ScheduleRow,
            "src/schedule/queries/list_for_workflow.sql",
            workflow.as_str()
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(Schedule::try_from)
        .collect()
    }

    async fn replace(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        version: Option<VersionNumber>,
        schedules: &[Schedule],
        consumers: &[QueueConsumer],
    ) -> Result<(), StoreError> {
        let mut tx = conn.begin().await?;
        let keep: Vec<String> = schedules
            .iter()
            .map(|schedule| schedule.trigger_id.as_str().to_owned())
            .collect();
        sqlx::query_file!(
            "src/schedule/queries/delete_others.sql",
            workflow.as_str(),
            &keep,
        )
        .execute(&mut *tx)
        .await?;
        for schedule in schedules {
            self.save(&mut tx, schedule).await?;
        }
        sqlx::query_file!(
            "src/schedule/queries/delete_consumers.sql",
            workflow.as_str()
        )
        .execute(&mut *tx)
        .await?;
        for consumer in consumers {
            let batch_size = consumer
                .batch_size
                .map(i32::try_from)
                .transpose()
                .map_err(|err| StoreError::invalid("batch size", err))?;
            sqlx::query_file!(
                "src/schedule/queries/save_consumer.sql",
                consumer.workflow_id.as_str(),
                consumer.trigger_id.as_str(),
                consumer.version.get(),
                consumer.queue.as_str(),
                batch_size,
            )
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query_file!(
            "src/schedule/queries/mark_synced.sql",
            workflow.as_str(),
            version.map(VersionNumber::get),
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn ready_consumers(
        &self,
        conn: &mut PgConnection,
        now: DateTime<Utc>,
    ) -> Result<Vec<ReadyConsumer>, StoreError> {
        sqlx::query_file!("src/schedule/queries/ready_consumers.sql", now)
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|row| {
                let consumer = QueueConsumer::try_from(ConsumerRow {
                    workflow_id: row.workflow_id,
                    trigger_id: row.trigger_id,
                    version: row.version,
                    queue: row.queue,
                    batch_size: row.batch_size,
                })?;
                let batch_size = u32::try_from(row.effective_batch_size)
                    .map_err(|err| StoreError::invalid("batch size", err))?;
                Ok(ReadyConsumer {
                    consumer,
                    batch_size,
                })
            })
            .collect()
    }

    async fn lock_active(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        version: VersionNumber,
    ) -> Result<bool, StoreError> {
        let locked = sqlx::query_file_scalar!(
            "src/schedule/queries/lock_active.sql",
            workflow.as_str(),
            version.get(),
        )
        .fetch_optional(&mut *conn)
        .await?;
        Ok(locked.is_some())
    }

    async fn claim_due(
        &self,
        conn: &mut PgConnection,
        now: DateTime<Utc>,
    ) -> Result<Option<DueSchedule>, StoreError> {
        let Some(row) = sqlx::query_file!("src/schedule/queries/claim_due.sql", now)
            .fetch_optional(&mut *conn)
            .await?
        else {
            return Ok(None);
        };
        let schedule = Schedule::try_from(ScheduleRow {
            workflow_id: row.workflow_id,
            trigger_id: row.trigger_id,
            version: row.version,
            expr: row.expr,
            tz: row.tz,
            jitter: row.jitter,
            overlap: row.overlap,
            catch_up: row.catch_up,
            next_at: row.next_at,
            due_at: row.due_at,
            queued: row.queued,
            last_run_id: row.last_run_id,
        })?;
        Ok(Some(DueSchedule {
            schedule,
            previous_active: row.previous_active,
        }))
    }

    async fn save(&self, conn: &mut PgConnection, schedule: &Schedule) -> Result<(), StoreError> {
        let trigger = &schedule.trigger;
        sqlx::query_file!(
            "src/schedule/queries/save.sql",
            schedule.workflow_id.as_str(),
            schedule.trigger_id.as_str(),
            schedule.version.get(),
            trigger.expr,
            trigger.tz,
            trigger.jitter.map(|jitter| jitter.to_string()),
            trigger.overlap_policy().as_str(),
            trigger.catch_up_policy().as_str(),
            schedule.next_at,
            schedule.due_at,
            &schedule.queued,
            schedule.last_run.map(RunId::as_uuid),
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    async fn delete(&self, conn: &mut PgConnection, schedule: &Schedule) -> Result<(), StoreError> {
        sqlx::query_file!(
            "src/schedule/queries/delete.sql",
            schedule.workflow_id.as_str(),
            schedule.trigger_id.as_str(),
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }
}
