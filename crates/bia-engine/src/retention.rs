//! Run retention (ADR-0021): a [`Chore`] that deletes Runs finished longer ago than their
//! retention, with their Timelines and leftover Jobs. Succeeded and cancelled Runs follow one
//! retention and failed Runs another, so failures can be kept longer for investigation
//! (ADR-0041). Unfinished Runs are never deleted, however old.

use bia_core::action::BoxFuture;
use bia_store::run::{PgRunRepository, RunRepository, RunStatus};
use chrono::{DateTime, TimeDelta, Utc};
use sqlx::PgPool;

use crate::{Chore, EngineError};

const FINISHED: &[RunStatus] = &[RunStatus::Succeeded, RunStatus::Cancelled];
const FAILED: &[RunStatus] = &[RunStatus::Failed];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionConfig {
    /// How long succeeded and cancelled Runs are kept after they finish.
    pub finished_runs: TimeDelta,
    /// How long failed Runs are kept after they finish.
    pub failed_runs: TimeDelta,
    /// How long Messages stay in a Dead-Letter Queue. Nothing reads it until Queues are
    /// stored.
    pub dead_letters: TimeDelta,
    pub interval: TimeDelta,
    /// Runs deleted per transaction, which bounds how long one holds its locks.
    pub batch_size: u32,
    /// Batches per retention kind in one run, so a backlog cannot hold up the scheduler; the
    /// rest waits for the next interval.
    pub batches_per_run: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            finished_runs: TimeDelta::days(90),
            failed_runs: TimeDelta::days(90),
            dead_letters: TimeDelta::days(90),
            interval: TimeDelta::minutes(10),
            batch_size: 500,
            batches_per_run: 20,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Retention {
    config: RetentionConfig,
}

impl Retention {
    pub fn new(config: RetentionConfig) -> Self {
        Self { config }
    }

    async fn delete_runs(
        &self,
        pool: &PgPool,
        statuses: &[RunStatus],
        finished_before: DateTime<Utc>,
    ) -> Result<u64, EngineError> {
        let mut deleted = 0;
        for _ in 0..self.config.batches_per_run {
            let mut conn = pool.acquire().await?;
            let batch = PgRunRepository
                .delete_finished(&mut conn, statuses, finished_before, self.config.batch_size)
                .await?;
            deleted += batch;
            if batch < u64::from(self.config.batch_size) {
                return Ok(deleted);
            }
        }
        tracing::info!(
            deleted,
            "more runs are past their retention; deleting them on the next run"
        );
        Ok(deleted)
    }
}

impl Chore for Retention {
    fn name(&self) -> &str {
        "retention"
    }

    fn interval(&self) -> TimeDelta {
        self.config.interval
    }

    fn run<'a>(
        &'a self,
        pool: &'a PgPool,
        now: DateTime<Utc>,
    ) -> BoxFuture<'a, Result<(), EngineError>> {
        Box::pin(async move {
            let finished = self
                .delete_runs(pool, FINISHED, now - self.config.finished_runs)
                .await?;
            let failed = self
                .delete_runs(pool, FAILED, now - self.config.failed_runs)
                .await?;
            if finished + failed > 0 {
                tracing::info!(finished, failed, "runs past their retention deleted");
            }
            Ok(())
        })
    }
}
