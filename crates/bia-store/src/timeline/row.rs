use bia_core::dsl::{ActionName, StepId};
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::{AttemptStatus, TimelineEntry};
use crate::StoreError;
use crate::run::RunId;

#[derive(Debug)]
pub(crate) struct TimelineRow {
    pub(crate) run_id: Uuid,
    pub(crate) seq: i32,
    pub(crate) step_id: String,
    pub(crate) iteration_path: String,
    pub(crate) attempt: i32,
    pub(crate) action: String,
    pub(crate) status: String,
    pub(crate) input: Option<Value>,
    pub(crate) output: Option<Value>,
    pub(crate) error: Option<Value>,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) finished_at: DateTime<Utc>,
}

impl TryFrom<TimelineRow> for TimelineEntry {
    type Error = StoreError;

    fn try_from(row: TimelineRow) -> Result<Self, StoreError> {
        Ok(Self {
            run_id: RunId::from_uuid(row.run_id),
            seq: row.seq,
            step_id: StepId::new(row.step_id).map_err(|err| StoreError::invalid("step id", err))?,
            iteration_path: row.iteration_path,
            attempt: row.attempt,
            action: ActionName::new(row.action)
                .map_err(|err| StoreError::invalid("action name", err))?,
            status: attempt_status(&row.status)?,
            input: row.input,
            output: row.output,
            error: row.error,
            started_at: row.started_at,
            finished_at: row.finished_at,
        })
    }
}

fn attempt_status(status: &str) -> Result<AttemptStatus, StoreError> {
    Ok(match status {
        "succeeded" => AttemptStatus::Succeeded,
        "failed" => AttemptStatus::Failed,
        "skipped" => AttemptStatus::Skipped,
        other => return Err(StoreError::invalid("attempt status", other)),
    })
}
