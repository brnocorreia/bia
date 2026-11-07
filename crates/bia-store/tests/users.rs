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
