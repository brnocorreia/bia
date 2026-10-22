use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::types::Json;
use uuid::Uuid;

use super::{Checkpoint, Run, RunId, RunStatus};
use crate::StoreError;
use crate::version::version_number;
use crate::workflow::workflow_id;

#[derive(Debug)]
pub(crate) struct RunRow {
    pub(crate) id: Uuid,
    pub(crate) workflow_id: String,
    pub(crate) version: i32,
    pub(crate) status: String,
    pub(crate) input: Value,
    pub(crate) trigger: Value,
    pub(crate) cursor: Json<Vec<Value>>,
    pub(crate) state: Value,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) finished_at: Option<DateTime<Utc>>,
    pub(crate) cancel_requested_at: Option<DateTime<Utc>>,
}

impl TryFrom<RunRow> for Run {
    type Error = StoreError;

    fn try_from(row: RunRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: RunId::from_uuid(row.id),
            workflow_id: workflow_id(row.workflow_id)?,
            version: version_number(row.version)?,
            status: run_status(&row.status)?,
            input: row.input,
            trigger: row.trigger,
            checkpoint: Checkpoint {
                cursor: row.cursor.0,
                state: row.state,
            },
            created_at: row.created_at,
            updated_at: row.updated_at,
            finished_at: row.finished_at,
            cancel_requested_at: row.cancel_requested_at,
        })
    }
}

fn run_status(status: &str) -> Result<RunStatus, StoreError> {
    Ok(match status {
        "pending" => RunStatus::Pending,
        "running" => RunStatus::Running,
        "sleeping" => RunStatus::Sleeping,
        "waiting" => RunStatus::Waiting,
        "succeeded" => RunStatus::Succeeded,
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        other => return Err(StoreError::invalid("run status", other)),
    })
}
