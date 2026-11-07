//! Migrations apply cleanly and are append-only.

#![allow(clippy::unwrap_used)]

use bia_store::workflow::{PgWorkflowRepository, WorkflowRepository};
use bia_store::{MIGRATOR, migrate, pending_migrations};
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
    (
        7,
        "60bc2b088ed0cf3e1b16583ceb9aa5b634d11650d2811882f88120015204964b2c90bd83653f4ffa30c5a9a8115f5f45",
    ),
    (
        8,
        "2286cbcbbdda50cde85d99c5ce34befd869c0c10ec5a00c7de0890c44780f013bd9c6a2d7a500e5428eee2782c652b75",
    ),
    (
        9,
        "ddecccb785e16c01e912b716cd5230e184ad4181bb5d542acaf57f5fbd042a83d777861a4ba767028819a6e12017b982",
    ),
    (
        10,
        "c9d48a60090d556ef0dc51f17b58b418c5afc1d2152a3fec68fe07d89a7eff0e640c21ce9e98864c980b17c552c6fb8f",
    ),
    (
        11,
        "9b4b92c2d0e3862dbfccc357227d36b7acd5edfe947a7d701505d3c490a3198c3a96bd98a260899843bb29ff8705e36f",
    ),
    (
        12,
        "87f96ce270b0051fb0c3dac093186ae7ad5e9808819675f42420ef811b65e2388ad3e2cd95e376661afd3aeb344758c8",
    ),
    (
        13,
        "b56be576a1a6d3b7ecdee1d0d21a3dbde974ef55034e5a0bf0af7fa9efcb0e169d50639d2447344e017f560dcf174e08",
    ),
    (
        14,
        "8cf1627a83391e9849e1ff00f96c82eb396c1b6a6e41f4336feac3f25a0c2a186890f387472abd976558c0fb54e9de64",
    ),
    (
        15,
        "07001c395ea995b4061769e396099a910bb80b7a3f7d8a335db02d7782b753937f63b9a1e56b0b69ffb3e0d9a57754bb",
    ),
    (
        16,
        "a648341cc25195a8d660767177a15888954afcdf69add67f570fc1d69beda3b5a7de1761d176359e42b990b01f630f45",
    ),
    (
        17,
        "43f312d28385678facdccf09eaa26b2cb03fba596116462d782a175ef05a7f1c9d98e353c7332073108d497d2c5d2c39",
    ),
];

#[sqlx::test(migrations = false)]
async fn every_migration_is_pending_until_migrate_runs(pool: PgPool) {
    let all: Vec<i64> = MIGRATOR.iter().map(|migration| migration.version).collect();
    assert_eq!(pending_migrations(&pool).await.unwrap(), all);

    migrate(&pool).await.unwrap();
    assert_eq!(pending_migrations(&pool).await.unwrap(), Vec::<i64>::new());

    let last = *all.last().unwrap();
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = $1")
        .bind(last)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(pending_migrations(&pool).await.unwrap(), vec![last]);
}

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

#[sqlx::test(migrations = false)]
async fn tokens_that_existed_before_origins_become_manual(pool: PgPool) {
    MIGRATOR.run_to(11, &pool).await.unwrap();
    let user: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO users (created_at) VALUES (now()) RETURNING id")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO tokens (name, hash, scope, user_id, created_at) \
         VALUES ('ci', decode(repeat('ab', 32), 'hex'), 'admin', $1, now())",
    )
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();

    migrate(&pool).await.unwrap();

    let origin: String = sqlx::query_scalar("SELECT origin FROM tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(origin, "manual");
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
