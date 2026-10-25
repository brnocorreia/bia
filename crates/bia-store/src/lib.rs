//! Postgres persistence: migrations, repositories, Queue and Job primitives.
//!
//! One module per domain (`workflow`, `version`, `run`, `timeline`, `job`, `schedule`,
//! `chore`, `token`), each laid out the same way:
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
use sqlx::migrate::{Migrate, MigrateError, Migrator};

pub mod chore;
mod clock;
mod error;
pub mod job;
pub mod run;
pub mod schedule;
pub mod timeline;
pub mod token;
pub mod version;
pub mod workflow;

pub use clock::SystemClock;
pub use error::StoreError;

pub static MIGRATOR: Migrator = sqlx::migrate!();

pub async fn migrate(pool: &PgPool) -> Result<(), StoreError> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

/// The versions of this build's migrations that the database has not applied successfully,
/// in order; empty once `bia migrate` has run. Reads only, so it never creates the migrations
/// table the way [`migrate`] does.
pub async fn pending_migrations(pool: &PgPool) -> Result<Vec<i64>, StoreError> {
    let mut conn = pool.acquire().await?;
    let (applied, dirty) = match conn.list_applied_migrations(&MIGRATOR.table_name).await {
        Ok(applied) => (applied, conn.dirty_version(&MIGRATOR.table_name).await?),
        Err(MigrateError::Execute(sqlx::Error::Database(error)))
            if error.code().as_deref() == Some(UNDEFINED_TABLE) =>
        {
            (Vec::new(), None)
        }
        Err(error) => return Err(error.into()),
    };
    Ok(MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| migration.version)
        .filter(|version| {
            Some(*version) == dirty || !applied.iter().any(|applied| applied.version == *version)
        })
        .collect())
}

const UNDEFINED_TABLE: &str = "42P01";
