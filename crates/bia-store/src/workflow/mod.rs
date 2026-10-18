//! Workflows: stable identities that own Versions and point at one Active Version.

mod model;
mod repository;
mod row;

pub use model::Workflow;
pub use repository::{PgWorkflowRepository, WorkflowRepository};
pub(crate) use row::workflow_id;
