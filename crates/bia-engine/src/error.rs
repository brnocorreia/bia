//! The error engine calls return when they cannot do their work, as opposed to a Step failing.

use bia_core::dsl::WorkflowId;
use bia_store::StoreError;
use bia_store::job::JobId;
use bia_store::run::RunId;

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
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
