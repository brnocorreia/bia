//! The error every repository method returns.

use bia_core::dsl::{QueueName, WorkflowId};

use crate::job::JobId;
use crate::queue::MessageId;
use crate::run::RunId;
use crate::version::VersionNumber;

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
    /// The Lease on the Job ended: another claimer took the Job after it expired, or the Job
    /// was already acked or failed.
    #[error("lease on job {0} was lost")]
    LeaseLost(JobId),
    #[error("queue `{0}` not found")]
    QueueNotFound(QueueName),
    /// A Queue names a Dead-Letter Queue that does not exist.
    #[error("dead-letter queue `{0}` not found")]
    DeadLetterQueueNotFound(QueueName),
    /// Another Queue names this one as its Dead-Letter Queue, so it cannot be deleted.
    #[error("queue `{0}` is another queue's dead-letter queue")]
    QueueInUse(QueueName),
    /// The delivery's Lease ended: the Message was delivered again after its visibility
    /// timeout ran out, or was already acked, retried or dead-lettered.
    #[error("lease on message {0} was lost")]
    MessageLeaseLost(MessageId),
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
