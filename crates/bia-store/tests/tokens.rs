//! Tokens carry what the Access screen lists: a Session's browser and an `oauth` Token's Grant.

#![allow(clippy::unwrap_used)]

use bia_store::token::{NewToken, Origin, PgTokenRepository, Scope, TokenHash, TokenRepository};
use bia_store::user::{PasswordHash, PgUserRepository, UserRepository};
use sqlx::PgPool;
use uuid::Uuid;

mod common;

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_oauth_token_lists_with_its_grant_and_a_session_with_its_browser(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let user = PgUserRepository
        .set_up(&mut conn, &PasswordHash("hash".into()), common::t0())
        .await
        .unwrap()
        .unwrap();
    let grant_id = Uuid::from_u128(7);
    let new = |byte: u8, origin, user_agent: Option<&str>, grant_id| NewToken {
        name: format!("{origin:?}"),
        hash: TokenHash([byte; 32]),
        scope: Scope::Agent,
        origin,
        user_id: user.id,
        created_at: common::at(i64::from(byte)),
        user_agent: user_agent.map(str::to_owned),
        grant_id,
    };
    PgTokenRepository
        .create(&mut conn, &new(1, Origin::OAuth, None, Some(grant_id)))
        .await
        .unwrap();
    PgTokenRepository
        .create(&mut conn, &new(2, Origin::Session, Some("Firefox"), None))
        .await
        .unwrap();
    PgTokenRepository
        .create(&mut conn, &new(3, Origin::Manual, None, None))
        .await
        .unwrap();

    let tokens = PgTokenRepository.list(&mut conn).await.unwrap();

    let details: Vec<_> = tokens
        .iter()
        .map(|t| (t.origin, t.user_agent.as_deref(), t.grant_id))
        .collect();
    assert_eq!(
        details,
        [
            (Origin::OAuth, None, Some(grant_id)),
            (Origin::Session, Some("Firefox"), None),
            (Origin::Manual, None, None),
        ]
    );
    let authenticated = PgTokenRepository
        .authenticate(&mut conn, TokenHash([1; 32]), common::t0())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(authenticated.grant_id, Some(grant_id));
}
