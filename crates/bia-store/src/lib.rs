//! Postgres persistence: migrations, repositories, Queue and Job primitives.
//!
//! One module per domain (`workflow`, `version`, `run`, `timeline`, `job`), each laid out the
//! same way:
//!
//! - `model.rs`: the values its repository takes and returns.
//! - `row.rs`: rows as Postgres returns them, converted into models with `TryFrom`.
//! - `repository.rs`: the repository trait and its Postgres implementation.
//! - `queries/`: one `.sql` file per statement, compile-time checked through
//!   `sqlx::query_file*!`.
//!
//! Repository methods take `&mut PgConnection`, so the same call works on a pooled connection
//! or inside a caller's transaction (ADR-0010). Methods that run several statements open a
//! savepoint, so they stay atomic either way. Every behavioural timestamp is a parameter the
//! caller reads from the injected `Clock`; no query uses Postgres `now()` for it (ADR-0022).
//! The table layout is ADR-0030.

use sqlx::PgPool;
use sqlx::migrate::Migrator;

mod error;
pub mod job;
pub mod run;
pub mod timeline;
pub mod version;
pub mod workflow;

pub use error::StoreError;

/// The schema migrations, append-only.
pub static MIGRATOR: Migrator = sqlx::migrate!();

/// Applies every pending migration.
pub async fn migrate(pool: &PgPool) -> Result<(), StoreError> {
    MIGRATOR.run(pool).await?;
    Ok(())
}
