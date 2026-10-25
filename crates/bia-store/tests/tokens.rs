//! Bootstrapping mints at most one admin Token, however many callers race for it.

#![allow(clippy::unwrap_used)]

use bia_store::token::{PgTokenRepository, Scope, TokenHash, TokenRepository};
use sqlx::PgPool;

mod common;

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn concurrent_first_admins_mint_exactly_one_token(pool: PgPool) {
    let attempts = (0..8u8).map(|n| {
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut conn = pool.acquire().await.unwrap();
            PgTokenRepository
                .create_first_admin(&mut conn, "admin", TokenHash([n; 32]), common::t0())
                .await
                .unwrap()
        })
    });
    let mut minted = Vec::new();
    for attempt in attempts {
        minted.extend(attempt.await.unwrap());
    }

    assert_eq!(minted.len(), 1, "{minted:?}");
    assert_eq!(minted[0].scope, Scope::Admin);
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(PgTokenRepository.list(&mut conn).await.unwrap(), minted);
}
