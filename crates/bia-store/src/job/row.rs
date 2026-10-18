use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::{Job, JobId, JobKind};
use crate::StoreError;
use crate::run::RunId;

#[derive(Debug)]
pub(crate) struct JobRow {
    pub(crate) id: i64,
    pub(crate) kind: String,
    pub(crate) run_id: Option<Uuid>,
    pub(crate) payload: Value,
    pub(crate) run_at: DateTime<Utc>,
    pub(crate) attempts: i32,
    pub(crate) max_attempts: i32,
    pub(crate) locked_by: Option<String>,
    pub(crate) locked_until: Option<DateTime<Utc>>,
    pub(crate) last_error: Option<String>,
    pub(crate) failed_at: Option<DateTime<Utc>>,
}

impl TryFrom<JobRow> for Job {
    type Error = StoreError;

    fn try_from(row: JobRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: JobId::new(row.id),
            kind: job_kind(&row.kind)?,
            run_id: row.run_id.map(RunId::from_uuid),
            payload: row.payload,
            run_at: row.run_at,
            attempts: row.attempts,
            max_attempts: row.max_attempts,
            locked_by: row.locked_by,
            locked_until: row.locked_until,
            last_error: row.last_error,
            failed_at: row.failed_at,
        })
    }
}

pub(crate) fn job_kind(kind: &str) -> Result<JobKind, StoreError> {
    match kind {
        "advance_run" => Ok(JobKind::AdvanceRun),
        other => Err(StoreError::invalid("job kind", other)),
    }
}
