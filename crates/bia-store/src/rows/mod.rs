//! Rows as Postgres returns them, and their conversion into models.
//!
//! Each row struct matches the columns of the queries that read it; `TryFrom<Row>` for the
//! model checks what the database cannot (ids, names, Definitions, status strings).

mod job;
mod run;
mod timeline;
mod version;
mod workflow;

pub(crate) use job::JobRow;
pub(crate) use run::RunRow;
pub(crate) use timeline::TimelineRow;
pub(crate) use version::{VersionRow, version_state};
pub(crate) use workflow::WorkflowRow;

use bia_core::dsl::WorkflowId;

use crate::StoreError;
use crate::models::VersionNumber;

fn workflow_id(id: String) -> Result<WorkflowId, StoreError> {
    WorkflowId::new(id).map_err(|err| StoreError::invalid("workflow id", err))
}

fn version_number(number: i32) -> Result<VersionNumber, StoreError> {
    VersionNumber::new(number).ok_or_else(|| StoreError::invalid("version number", number))
}
