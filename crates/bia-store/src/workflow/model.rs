use bia_core::dsl::WorkflowId;
use chrono::{DateTime, Utc};

use crate::version::VersionNumber;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub id: WorkflowId,
    pub active_version: Option<VersionNumber>,
    pub created_at: DateTime<Utc>,
}
