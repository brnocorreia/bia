//! First-run setup: the code from the Setup Link and a password create the install's User,
//! once. Until sign-in exists, setup also returns an admin Token.

#![allow(clippy::unwrap_used)]

use bia_client::{Error, Scope};
use bia_store::user::{PgUserRepository, UserRepository};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

use common::{PASSWORD, TestServer};

/// The status and code of a failed call.
fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> (StatusCode, String) {
    match result {
        Err(Error::Api { status, body }) => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    }
}

/// Posts `body` to `/v1/setup` and returns the status and the raw response text.
async fn post_setup(server: &TestServer, body: Value) -> (StatusCode, String) {
    let response = reqwest::Client::new()
        .post(server.url("/v1/setup"))
        .json(&body)
        .send()
        .await
        .unwrap();
    (response.status(), response.text().await.unwrap())
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn setup_creates_the_user_and_returns_an_admin_token(pool: PgPool) {
    let server = TestServer::without_tokens(pool.clone()).await;
    let mut conn = pool.acquire().await.unwrap();
    assert!(PgUserRepository.needs_setup(&mut conn).await.unwrap());

    let done = server
        .anonymous()
        .set_up(&server.setup_code, PASSWORD)
        .await
        .unwrap();

    assert_eq!(done.user.created_at, common::t0());
    assert_eq!(done.token.token.scope, Scope::Admin);
    let tokens = server
        .client_with(&done.token.value)
        .list_tokens()
        .await
        .unwrap();
    let ids: Vec<_> = tokens.iter().map(|token| token.id).collect();
    assert_eq!(ids, [done.token.token.id]);
    assert!(!PgUserRepository.needs_setup(&mut conn).await.unwrap());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_second_setup_is_refused_with_already_set_up(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let again = server
        .anonymous()
        .set_up(&server.setup_code, "another-password-entirely")
        .await;

    assert_eq!(
        failure(again),
        (StatusCode::CONFLICT, "already_set_up".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_wrong_code_is_refused_with_invalid_setup_code(pool: PgPool) {
    let server = TestServer::without_tokens(pool).await;
    let client = server.anonymous();
    let (start, secret) = server.setup_code.split_once('-').unwrap();
    let flipped = if secret.starts_with('0') { "1" } else { "0" };
    let same_start = format!("{start}-{flipped}{}", &secret[1..]);

    for code in [same_start.as_str(), "", "not-a-code"] {
        assert_eq!(
            failure(client.set_up(code, PASSWORD).await),
            (StatusCode::FORBIDDEN, "invalid_setup_code".into()),
            "{code:?}"
        );
    }
    client.set_up(&server.setup_code, PASSWORD).await.unwrap();
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_code_from_before_a_restart_is_refused_with_setup_code_expired(pool: PgPool) {
    let before = TestServer::without_tokens(pool.clone()).await;
    let old_code = before.setup_code.clone();
    drop(before);
    let restarted = TestServer::without_tokens(pool).await;

    let result = restarted.anonymous().set_up(&old_code, PASSWORD).await;

    assert_eq!(
        failure(result),
        (StatusCode::FORBIDDEN, "setup_code_expired".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_password_under_12_characters_is_refused_naming_the_rule(pool: PgPool) {
    let server = TestServer::without_tokens(pool).await;
    // 11 characters in 22 bytes: the rule counts characters.
    let short = "ééééééééééé";

    let (status, body) = post_setup(
        &server,
        json!({ "code": server.setup_code, "password": short }),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["code"], "password_too_short");
    assert!(
        body["message"].as_str().unwrap().contains("12 characters"),
        "{body}"
    );
    server
        .anonymous()
        .set_up(&server.setup_code, "twelve chars")
        .await
        .unwrap();
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn error_bodies_never_quote_the_code_or_the_password(pool: PgPool) {
    let server = TestServer::without_tokens(pool).await;
    let code = server.setup_code.clone();
    let wrong_code = format!("{}-{}", "f".repeat(8), "e".repeat(32));
    let refusals = [
        json!({ "code": wrong_code, "password": "a-secret-password" }),
        json!({ "code": code, "password": "secret" }),
        // serde quotes a value of the wrong type in its own errors.
        json!({ "code": code, "password": 314159265358979_u64 }),
        json!({ "code": code, "password": "a-secret-password", "extra": 1 }),
    ];

    for body in refusals {
        let (status, text) = post_setup(&server, body.clone()).await;
        assert!(status.is_client_error(), "{status} {text}");
        for value in body.as_object().unwrap().values() {
            let value = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            assert!(!text.contains(&value), "{text} quotes {value}");
        }
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn resetting_the_admin_revokes_every_token_and_a_new_setup_link_works(pool: PgPool) {
    let server = TestServer::start(pool.clone()).await;
    let agent = server.token(Scope::Agent).await;
    let hook = server.webhook_token("onboarding-email", "signup").await;
    drop(server);

    // What `bia serve --reset-admin` does before it serves.
    let mut conn = pool.acquire().await.unwrap();
    let revoked = PgUserRepository
        .reset(&mut conn, common::t0())
        .await
        .unwrap();
    assert_eq!(revoked, Some(3));
    assert!(PgUserRepository.needs_setup(&mut conn).await.unwrap());

    let restarted = TestServer::without_tokens(pool).await;
    for token in [&agent, &hook] {
        assert_eq!(
            failure(restarted.client_with(token).list_workflows().await),
            (StatusCode::UNAUTHORIZED, "invalid_token".into())
        );
    }
    let done = restarted
        .anonymous()
        .set_up(&restarted.setup_code, "a-brand-new-password")
        .await
        .unwrap();
    let tokens = restarted
        .client_with(&done.token.value)
        .list_tokens()
        .await
        .unwrap();
    assert_eq!(tokens.len(), 4);
    assert_eq!(
        tokens.iter().filter(|t| t.revoked_at.is_none()).count(),
        1,
        "{tokens:?}"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn resetting_without_a_user_changes_nothing(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();

    let revoked = PgUserRepository
        .reset(&mut conn, common::t0())
        .await
        .unwrap();

    assert_eq!(revoked, None);
    assert!(PgUserRepository.needs_setup(&mut conn).await.unwrap());
}
