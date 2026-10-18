use bia_core::dsl::WorkflowId;
use chrono::{DateTime, Utc};

use super::Workflow;
use crate::StoreError;
use crate::version::version_number;

#[derive(Debug)]
pub(crate) struct WorkflowRow {
    pub(crate) id: String,
    pub(crate) active_version: Option<i32>,
    pub(crate) created_at: DateTime<Utc>,
}

impl TryFrom<WorkflowRow> for Workflow {
    type Error = StoreError;

    fn try_from(row: WorkflowRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: workflow_id(row.id)?,
            active_version: row.active_version.map(version_number).transpose()?,
            created_at: row.created_at,
        })
    }
}

/// Reads back a stored Workflow id.
pub(crate) fn workflow_id(id: String) -> Result<WorkflowId, StoreError> {
    WorkflowId::new(id).map_err(|err| StoreError::invalid("workflow id", err))
}
