//! Migrations apply cleanly and are append-only.

#![allow(clippy::unwrap_used)]

use bia_store::workflow::{PgWorkflowRepository, WorkflowRepository};
use bia_store::{MIGRATOR, migrate};
use sqlx::PgPool;

mod common;

/// Every migration ever released, with its checksum. Editing a released migration changes its
/// checksum and fails this test; add a new migration and append it here instead.
const RELEASED: &[(i64, &str)] = &[
    (
        1,
        "0be8b42ff67613f91d99a00ac79f61ae67191ce1f650693a853e677741cb3519c8676a9de8d12e2a181a182859099a99",
    ),
    (
        2,
        "2b89dcea7a66feb844e941a62ada571014c8b58d2daca25db9fe9dd3e0dfaf09d609200ee348008aea492d74473c22fc",
    ),
    (
        3,
        "30cc7fa39e34e01c95fccea5584f759121d7ef195a8d8ab9b9d76863a17e916cc6b54c9a5df8fad34639c013adf12e72",
    ),
    (
        4,
        "1ee5eccef3b8672b4db6e0131390c1df4ba2db2f1908b93f2b7140f89e73b7f394a94d34f93cfcd3ad6c4c092962193a",
    ),
    (
        5,
        "a19e2d616ef6b641e789d96d3f79af32ba09d5c0598fd4a9b0b2a1b74dcc60dcb31579d9a9ff496b7059b818ad95165f",
    ),
    (
        6,
        "8ee87e0dcff266208a87c8297bc8bc13cdeaa1042dba2a12f6c6b5c0a4c4a9bed3c9c1a41f377acba9e2d7c7325d4218",
    ),
];

#[sqlx::test(migrations = false)]
async fn migrations_apply_to_an_empty_database(pool: PgPool) {
    migrate(&pool).await.unwrap();
    migrate(&pool).await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    PgWorkflowRepository
        .create(&mut conn, &common::workflow_id("onboarding-email"))
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
