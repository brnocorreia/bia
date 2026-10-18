//! Postgres persistence: migrations, repositories, Queue and Job primitives.
//!
//! - `queries/` (next to `src/`): one `.sql` file per statement.
//! - [`models`]: the values repositories take and return.
//! - `rows`: rows as Postgres returns them, converted into models with `TryFrom`.
//! - [`repositories`]: one trait per aggregate and its Postgres implementation.
//!
//! Every behavioural timestamp is a parameter the caller reads from the injected `Clock`;
//! no query uses Postgres `now()` for it (ADR-0022). The table layout is ADR-0030.

use sqlx::PgPool;
use sqlx::migrate::Migrator;

mod error;
pub mod models;
pub mod repositories;
mod rows;

pub use error::StoreError;

/// The schema migrations, append-only.
pub static MIGRATOR: Migrator = sqlx::migrate!();

/// Applies every pending migration.
pub async fn migrate(pool: &PgPool) -> Result<(), StoreError> {
    MIGRATOR.run(pool).await?;
    Ok(())
}
