//! The error engine calls return when they cannot do their work, as opposed to a Step failing.

use bia_core::dsl::WorkflowId;
use bia_store::StoreError;
use bia_store::job::JobId;
use bia_store::run::{RunId, RunStatus};

/// Why the engine could not advance a Run. The Job stays claimable again, so the Worker loop
/// fails it with a backoff; a Step failing is not one of these but a recorded transition.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    #[error("job {0} does not name a run")]
    JobWithoutRun(JobId),
    #[error("run {0} not found")]
    RunNotFound(RunId),
    #[error("version {version} of workflow `{workflow}`, which run {run} is pinned to, not found")]
    VersionNotFound {
        run: RunId,
        workflow: WorkflowId,
        version: i32,
    },
    #[error("checkpoint of run {run} is invalid: {message}")]
    InvalidCheckpoint { run: RunId, message: String },
    #[error("run {run} cannot be resumed: {reason}")]
    NotResumable { run: RunId, reason: NotResumable },
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Why a Run cannot be resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NotResumable {
    /// Only a `failed` Run resumes.
    #[error("it is {}, not failed", .0.as_str())]
    NotFailed(RunStatus),
    /// The Run failed before any Step ran, e.g. on invalid input, so there is no Step to
    /// resume from.
    #[error("it failed before any Step ran")]
    NoStep,
}
