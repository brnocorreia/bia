use chrono::{DateTime, Utc};

use crate::StoreError;
use crate::models::Workflow;

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
            id: super::workflow_id(row.id)?,
            active_version: row.active_version.map(super::version_number).transpose()?,
            created_at: row.created_at,
        })
    }
}
