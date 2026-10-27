//! Setup creates at most one User, however many callers race for it.

#![allow(clippy::unwrap_used)]

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
