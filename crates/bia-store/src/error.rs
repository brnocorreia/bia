//! The error every repository function returns.

use bia_core::dsl::WorkflowId;

use crate::jobs::JobId;
use crate::runs::RunId;
use crate::versions::VersionNumber;

/// Why a repository call failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    #[error("workflow `{0}` already exists")]
    WorkflowExists(WorkflowId),
    #[error("workflow `{0}` not found")]
    WorkflowNotFound(WorkflowId),
    #[error("version {number} of workflow `{workflow}` not found")]
    VersionNotFound {
        workflow: WorkflowId,
        number: VersionNumber,
    },
    /// The Version is Published, so it can no longer be replaced or published again.
    #[error("version {number} of workflow `{workflow}` is published and cannot be modified")]
    VersionPublished {
        workflow: WorkflowId,
        number: VersionNumber,
    },
    /// The Version is a Draft, so it cannot become the Active Version or start Runs.
    #[error("version {number} of workflow `{workflow}` is a draft")]
    VersionNotPublished {
        workflow: WorkflowId,
        number: VersionNumber,
    },
    /// A Definition saved under a Workflow whose id it does not carry.
    #[error("definition id `{found}` does not match workflow `{workflow}`")]
    DefinitionMismatch {
        workflow: WorkflowId,
        found: WorkflowId,
    },
    #[error("run {0} not found")]
    RunNotFound(RunId),
    #[error("job {0} not found")]
    JobNotFound(JobId),
    /// A stored value this build cannot read back.
    #[error("stored {what} is invalid: {message}")]
    InvalidStoredData { what: &'static str, message: String },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

impl StoreError {
    pub(crate) fn invalid(what: &'static str, message: impl ToString) -> Self {
        Self::InvalidStoredData {
            what,
            message: message.to_string(),
        }
    }
}
