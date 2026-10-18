//! Migrations apply cleanly and are append-only.

// `allow-unwrap-in-tests` covers `#[test]` functions only, not the helpers they share.
#![allow(clippy::unwrap_used)]

use bia_store::{MIGRATOR, migrate, workflows};
use sqlx::PgPool;

mod common;

/// Every migration ever released, with its checksum. Editing a released migration changes its
/// checksum and fails this test; add a new migration and append it here instead.
const RELEASED: &[(i64, &str)] = &[(
    1,
    "a0c7898edb55d3417d0fe8b632d02d998aebc5bd65f5f9e375704b1e5ad3992ff90af209ae1feaf0c876d2c2bd33cfe7",
)];

#[sqlx::test(migrations = false)]
async fn migrations_apply_to_an_empty_database(pool: PgPool) {
    migrate(&pool).await.unwrap();
    // A second run finds nothing pending.
    migrate(&pool).await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    workflows::create(&mut conn, &common::workflow_id("onboarding-email"))
        .await
        .unwrap();
}

#[test]
fn released_migrations_are_unchanged() {
    let migrations: Vec<(i64, String)> = MIGRATOR
        .iter()
        .map(|migration| {
            let checksum = migration
                .checksum
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            (migration.version, checksum)
        })
        .collect();
    let released: Vec<(i64, String)> = RELEASED
        .iter()
        .map(|(version, checksum)| (*version, (*checksum).to_owned()))
        .collect();

    assert_eq!(migrations, released);
}
