//! Authorization codes and refresh tokens are single-use however many callers race for them.

#![allow(clippy::unwrap_used)]

use bia_store::oauth::{
    CodeRedemption, GrantScope, NewApproval, NewAuthorization, NewGrant, OAuthRepository,
    PgOAuthRepository, Provenance, RefreshRedemption, SecretHash,
};
use bia_store::user::{PasswordHash, PgUserRepository, UserRepository};
use chrono::TimeDelta;
use sqlx::PgPool;

mod common;

async fn user(pool: &PgPool) -> bia_store::user::User {
    let mut conn = pool.acquire().await.unwrap();
    PgUserRepository
        .set_up(&mut conn, &PasswordHash("hash".into()), common::t0())
        .await
        .unwrap()
        .unwrap()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_code_redeems_once_even_when_two_callers_race_for_it(pool: PgPool) {
    let user = user(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let pending = PgOAuthRepository
        .begin_authorization(
            &mut conn,
            &NewAuthorization {
                client_id: "client".into(),
                client_name: "Client".into(),
                client_host: None,
                provenance: Provenance::Dynamic,
                redirect_uri: "http://127.0.0.1/cb".into(),
                requested_scope: GrantScope::Agent,
                resource: "http://bia.test".into(),
                state: None,
                code_challenge: "c".repeat(43),
                created_at: common::t0(),
                expires_at: common::t0() + TimeDelta::minutes(10),
            },
        )
        .await
        .unwrap();
    let code = SecretHash([9; 32]);
    PgOAuthRepository
        .approve(
            &mut conn,
            pending.id,
            &NewApproval {
                user_id: user.id,
                scope: GrantScope::Agent,
                code,
                now: common::t0(),
                code_expires_at: common::t0() + TimeDelta::seconds(60),
            },
        )
        .await
        .unwrap()
        .unwrap();

    let redeem = || {
        let pool = pool.clone();
        async move {
            let mut conn = pool.acquire().await.unwrap();
            PgOAuthRepository
                .redeem_code(&mut conn, code, common::at(1))
                .await
                .unwrap()
        }
    };
    let (a, b) = tokio::join!(redeem(), redeem());

    let redeemed = [a, b]
        .iter()
        .filter(|r| matches!(r, CodeRedemption::Redeemed(_)))
        .count();
    assert_eq!(redeemed, 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_refresh_token_redeems_once_and_a_second_use_revokes_the_grant(pool: PgPool) {
    let user = user(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let grant = PgOAuthRepository
        .create_grant(
            &mut conn,
            &NewGrant {
                user_id: user.id,
                client_id: "client".into(),
                client_name: "Client".into(),
                client_host: None,
                provenance: Provenance::Cimd,
                scope: GrantScope::Agent,
                resource: "http://bia.test".into(),
                created_at: common::t0(),
            },
        )
        .await
        .unwrap();
    let hash = SecretHash([5; 32]);
    PgOAuthRepository
        .add_refresh_token(
            &mut conn,
            grant.id,
            hash,
            common::t0(),
            common::t0() + TimeDelta::days(90),
        )
        .await
        .unwrap();

    let first = PgOAuthRepository
        .redeem_refresh_token(&mut conn, hash, common::at(1))
        .await
        .unwrap();
    let second = PgOAuthRepository
        .redeem_refresh_token(&mut conn, hash, common::at(2))
        .await
        .unwrap();
    let third = PgOAuthRepository
        .redeem_refresh_token(&mut conn, hash, common::at(3))
        .await
        .unwrap();

    assert!(matches!(first, RefreshRedemption::Redeemed(_)));
    assert!(matches!(second, RefreshRedemption::Reused(_)));
    assert!(matches!(third, RefreshRedemption::Revoked));
    let grant = PgOAuthRepository
        .grant(&mut conn, grant.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grant.revoked_at, Some(common::at(2)));
}
