//! Setup creates at most one User, however many callers race for it, and a reset takes their
//! passkeys away.

#![allow(clippy::unwrap_used)]

use bia_store::passkey::{CeremonyKind, NewPasskey, PasskeyRepository, PgPasskeyRepository};
use bia_store::user::{PasswordHash, PgUserRepository, UserRepository};
use sqlx::PgPool;

mod common;

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn concurrent_setups_create_exactly_one_user(pool: PgPool) {
    let attempts = (0..8).map(|n| {
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut conn = pool.acquire().await.unwrap();
            PgUserRepository
                .set_up(&mut conn, &PasswordHash(format!("hash-{n}")), common::t0())
                .await
                .unwrap()
        })
    });
    let mut created = Vec::new();
    for attempt in attempts {
        created.extend(attempt.await.unwrap());
    }

    assert_eq!(created.len(), 1, "{created:?}");
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn after_a_reset_setup_gives_the_same_user_a_password_again(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let user = PgUserRepository
        .set_up(&mut conn, &PasswordHash("first".into()), common::t0())
        .await
        .unwrap()
        .unwrap();
    PgUserRepository
        .reset(&mut conn, common::t0())
        .await
        .unwrap();

    let again = PgUserRepository
        .set_up(&mut conn, &PasswordHash("second".into()), common::t0())
        .await
        .unwrap();

    assert_eq!(again, Some(user));
    let refused = PgUserRepository
        .set_up(&mut conn, &PasswordHash("third".into()), common::t0())
        .await
        .unwrap();
    assert_eq!(refused, None);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_reset_deletes_the_passkeys_and_unfinished_ceremonies(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let user = PgUserRepository
        .set_up(&mut conn, &PasswordHash("hash".into()), common::t0())
        .await
        .unwrap()
        .unwrap();
    PgPasskeyRepository
        .create(
            &mut conn,
            &NewPasskey {
                user_id: user.id,
                name: "laptop".into(),
                credential: serde_json::json!({}),
                created_at: common::t0(),
            },
        )
        .await
        .unwrap();
    PgPasskeyRepository
        .begin_ceremony(
            &mut conn,
            user.id,
            CeremonyKind::Authentication,
            &serde_json::json!({}),
            common::t0(),
            common::t0() + chrono::TimeDelta::minutes(5),
        )
        .await
        .unwrap();

    PgUserRepository
        .reset(&mut conn, common::t0())
        .await
        .unwrap();

    let passkeys: i64 = sqlx::query_scalar("SELECT count(*) FROM passkeys")
        .fetch_one(&pool)
        .await
        .unwrap();
    let ceremonies: i64 = sqlx::query_scalar("SELECT count(*) FROM passkey_ceremonies")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((passkeys, ceremonies), (0, 0));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_reset_revokes_grants_and_closes_every_open_authorization_and_device_code(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let user = PgUserRepository
        .set_up(&mut conn, &PasswordHash("hash".into()), common::t0())
        .await
        .unwrap()
        .unwrap();
    let grant: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO grants (user_id, client_id, client_name, provenance, scope, resource, created_at)
         VALUES ($1, 'c', 'Client', 'dynamic', 'agent', 'http://bia.test', $2) RETURNING id",
    )
    .bind(user.id.as_uuid())
    .bind(common::t0())
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO oauth_authorizations
           (client_id, client_name, provenance, redirect_uri, requested_scope, resource,
            code_challenge, created_at, expires_at)
         VALUES ('c', 'Client', 'dynamic', 'http://127.0.0.1/cb', 'agent', 'http://bia.test',
                 'x', $1, $1 + interval '10 minutes')",
    )
    .bind(common::t0())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO oauth_device_authorizations
           (client_id, client_name, provenance, requested_scope, resource, device_code_hash,
            user_code_hash, created_at, expires_at, interval_secs)
         VALUES ('c', 'Client', 'builtin', 'admin', 'http://bia.test', decode(repeat('01', 32), 'hex'),
                 decode(repeat('02', 32), 'hex'), $1, $1 + interval '10 minutes', 5)",
    )
    .bind(common::t0())
    .execute(&pool)
    .await
    .unwrap();

    PgUserRepository
        .reset(&mut conn, common::t0())
        .await
        .unwrap();

    let revoked: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM grants WHERE id = $1")
            .bind(grant)
            .fetch_one(&pool)
            .await
            .unwrap();
    let open_authorizations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM oauth_authorizations WHERE code_used_at IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    let open_device_codes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM oauth_device_authorizations WHERE consumed_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(revoked.is_some());
    assert_eq!((open_authorizations, open_device_codes), (0, 0));
}
