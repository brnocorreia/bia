//! Runs: executions pinned to the Version they started on (ADR-0009), and their Checkpoints.

mod model;
mod repository;
mod row;

pub use model::{Checkpoint, NewRun, Run, RunFilter, RunId, RunStatus};
pub use repository::{PgRunRepository, RunRepository};
