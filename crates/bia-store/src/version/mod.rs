//! Versions: immutable, numbered snapshots of a Workflow's Definition (ADR-0009).

mod model;
mod repository;
mod row;

pub use model::{Version, VersionNumber, VersionState};
pub(crate) use repository::why_not_published;
pub use repository::{PgVersionRepository, VersionRepository};
pub(crate) use row::version_number;
