//! Repository traits and their Postgres implementations.
//!
//! Every method takes `&mut PgConnection`, so the same call works on a pooled connection or
//! inside a caller's transaction (ADR-0010). Methods that run several statements open a
//! savepoint, so they stay atomic either way. The SQL lives in `queries/`, one file per
//! statement, checked at compile time against the schema through `sqlx::query_file*!`.

mod job;
mod run;
mod timeline;
mod version;
mod workflow;

pub use job::{JobRepository, PgJobRepository};
pub use run::{PgRunRepository, RunRepository};
pub use timeline::{PgTimelineRepository, TimelineRepository};
pub use version::{PgVersionRepository, VersionRepository};
pub use workflow::{PgWorkflowRepository, WorkflowRepository};
