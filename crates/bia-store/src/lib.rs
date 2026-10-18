//! Postgres persistence: migrations, repositories, Queue and Job primitives.
//!
//! Repositories are free functions over `&mut PgConnection`, so the same call works on a
//! pooled connection or inside a caller's transaction: a Step's Checkpoint, its Timeline
//! entry, its Job completion and the next Job commit together (ADR-0010). Functions that run
//! several statements open a savepoint, so they stay atomic either way.
//!
//! Every behavioural timestamp is a parameter the caller reads from the injected `Clock`;
//! no query uses Postgres `now()` for it (ADR-0022). The table layout is ADR-0030.

use sqlx::PgPool;
use sqlx::migrate::Migrator;

mod error;
pub mod jobs;
pub mod runs;
pub mod timeline;
pub mod versions;
pub mod workflows;

pub use error::StoreError;

/// The schema migrations, append-only.
pub static MIGRATOR: Migrator = sqlx::migrate!();

/// Applies every pending migration.
pub async fn migrate(pool: &PgPool) -> Result<(), StoreError> {
    MIGRATOR.run(pool).await?;
    Ok(())
}
