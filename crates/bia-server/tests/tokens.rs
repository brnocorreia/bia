//! Every request but the probes and bootstrapping is authenticated by a Token, whose Scope
//! decides which routes it reaches; Token values are shown once and stored only as hashes.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode as AxumStatus;
use axum::routing::post;
use bia_client::{Error, NewToken, Scope, WebhookBinding};
use bia_core::clock::ManualClock;
use bia_server::auth::Webhook;
use bia_server::{ApiConfig, ApiState};
use chrono::TimeDelta;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

use common::{BOOTSTRAP_KEY, TestServer};

/// The status and code of a failed call.
fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> (StatusCode, String) {
    match result {
        Err(Error::Api { status, body }) => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    }
}

/// An unknown value with the shape of a Token.
const UNKNOWN: &str = "bia_0000000000000000000000000000000000000000000000000000000000000000";

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn requests_without_a_token_or_with_an_unknown_or_revoked_one_are_unauthenticated(
    pool: PgPool,
) {
    let server = TestServer::start(pool).await;

    let missing = server.anonymous().schema().await;
    assert_eq!(
        failure(missing),
        (StatusCode::UNAUTHORIZED, "missing_token".into())
    );
    for unknown in [UNKNOWN, "not-a-token"] {
        let result = server.client_with(unknown).schema().await;
        assert_eq!(
            failure(result),
            (StatusCode::UNAUTHORIZED, "invalid_token".into()),
            "{unknown}"
        );
    }
    let basic = reqwest::Client::new()
        .get(server.url("/v1/schema"))
        .header("authorization", "Basic YWRtaW46YWRtaW4=")
        .send()
        .await
        .unwrap();
    assert_eq!(basic.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(basic.headers()["www-authenticate"], "Bearer");

    let agent = server.created_token(Scope::Agent).await;
    server.client_with(&agent.value).schema().await.unwrap();
    let revoked = server.client.revoke_token(agent.token.id).await.unwrap();
    assert_eq!(revoked.revoked_at, Some(common::t0()));
    let result = server.client_with(&agent.value).schema().await;
    assert_eq!(
        failure(result),
        (StatusCode::UNAUTHORIZED, "invalid_token".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_agent_token_is_forbidden_on_admin_routes_and_an_admin_token_passes(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let agent = server.client_with(&server.token(Scope::Agent).await);

    agent.schema().await.unwrap();
    assert_eq!(
        failure(agent.list_tokens().await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );
    let escalate = NewToken {
        name: "escalate".into(),
        scope: Scope::Admin,
        webhook: None,
    };
    assert_eq!(
        failure(agent.create_token(&escalate).await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );

    server.client.schema().await.unwrap();
    assert_eq!(server.client.list_tokens().await.unwrap().len(), 2);
}

/// A server with a stand-in webhook route at `/hooks/{workflow_id}/{trigger_id}`.
async fn server_with_a_webhook_route(pool: PgPool) -> TestServer {
    async fn hook(_: Webhook) -> AxumStatus {
        AxumStatus::ACCEPTED
    }
    let clock = ManualClock::new(common::t0());
    let state = common::state(pool, &clock, ApiConfig::default());
    let hooks = Router::new()
        .route("/hooks/{workflow_id}/{trigger_id}", post(hook))
        .with_state(state.clone());
    TestServer::bootstrapped(bia_server::router(state).merge(hooks), clock).await
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_webhook_token_reaches_only_its_own_webhook_route(pool: PgPool) {
    let server = server_with_a_webhook_route(pool).await;
    let webhook = server.webhook_token("onboarding-email", "signup").await;
    let agent = server.token(Scope::Agent).await;
    let http = reqwest::Client::new();
    let call = |path: &str, token: &str| http.post(server.url(path)).bearer_auth(token).send();

    let own = call("/hooks/onboarding-email/signup", &webhook)
        .await
        .unwrap();
    assert_eq!(own.status(), StatusCode::ACCEPTED);
    for other in [
        "/hooks/onboarding-email/other",
        "/hooks/other-workflow/signup",
    ] {
        let response = call(other, &webhook).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{other}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["code"], "insufficient_scope");
    }
    let client = server.client_with(&webhook);
    assert_eq!(
        failure(client.schema().await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );
    assert_eq!(
        failure(client.list_tokens().await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );

    let by_agent = call("/hooks/onboarding-email/signup", &agent)
        .await
        .unwrap();
    assert_eq!(by_agent.status(), StatusCode::FORBIDDEN);
    let by_admin = call("/hooks/onboarding-email/signup", &server.admin_token)
        .await
        .unwrap();
    assert_eq!(by_admin.status(), StatusCode::ACCEPTED);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn token_values_are_shown_once_and_stored_only_as_hashes(pool: PgPool) {
    let server = TestServer::start(pool.clone()).await;

    let created = server
        .client
        .create_token(&NewToken {
            name: "claude-desktop".into(),
            scope: Scope::Webhook,
            webhook: Some(WebhookBinding {
                workflow_id: "onboarding-email".into(),
                trigger_id: "signup".into(),
            }),
        })
        .await
        .unwrap();
    assert!(created.value.starts_with("bia_"), "{}", created.value);
    assert_eq!(created.token.name, "claude-desktop");
    assert_eq!(created.token.scope, Scope::Webhook);
    assert_eq!(created.token.created_at, common::t0());
    assert!(!format!("{created:?}").contains(&created.value));

    let listed = reqwest::Client::new()
        .get(server.url("/v1/tokens"))
        .bearer_auth(&server.admin_token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let rejected = reqwest::Client::new()
        .get(server.url("/v1/tokens"))
        .bearer_auth(UNKNOWN)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for value in [&created.value, &server.admin_token] {
        assert!(!listed.contains(value.as_str()), "{listed}");
        let stored: i64 =
            sqlx::query_scalar("SELECT count(*) FROM tokens t WHERE t::text LIKE '%' || $1 || '%'")
                .bind(value)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, 0, "a column holds the value");
        let hashed: i64 =
            sqlx::query_scalar("SELECT count(*) FROM tokens WHERE hash = sha256($1::bytea)")
                .bind(value.as_bytes())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(hashed, 1);
    }
    assert!(!rejected.contains(UNKNOWN), "{rejected}");

    let tokens: Value = serde_json::from_str(&listed).unwrap();
    let webhook = tokens["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|token| token["name"] == "claude-desktop")
        .unwrap();
    assert_eq!(
        webhook["webhook"],
        json!({ "workflow_id": "onboarding-email", "trigger_id": "signup" })
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_tokens_last_use_is_recorded_to_the_minute(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let created = server.created_token(Scope::Agent).await;
    let agent = server.client_with(&created.value);
    let last_used = || async {
        let tokens = server.client.list_tokens().await.unwrap();
        let token = tokens.into_iter().find(|t| t.id == created.token.id);
        token.unwrap().last_used_at
    };
    assert_eq!(last_used().await, None);

    agent.schema().await.unwrap();
    assert_eq!(last_used().await, Some(common::t0()));

    server.clock.advance(TimeDelta::seconds(30));
    agent.schema().await.unwrap();
    assert_eq!(last_used().await, Some(common::t0()));

    server.clock.advance(TimeDelta::seconds(31));
    agent.schema().await.unwrap();
    assert_eq!(
        last_used().await,
        Some(common::t0() + TimeDelta::seconds(61))
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_token_request_with_a_misplaced_or_missing_webhook_binding_is_malformed(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let binding = WebhookBinding {
        workflow_id: "onboarding-email".into(),
        trigger_id: "signup".into(),
    };
    let invalid = [
        NewToken {
            name: "hook".into(),
            scope: Scope::Webhook,
            webhook: None,
        },
        NewToken {
            name: "agent".into(),
            scope: Scope::Agent,
            webhook: Some(binding.clone()),
        },
        NewToken {
            name: " ".into(),
            scope: Scope::Agent,
            webhook: None,
        },
        NewToken {
            name: "hook".into(),
            scope: Scope::Webhook,
            webhook: Some(WebhookBinding {
                workflow_id: "Not A Workflow".into(),
                ..binding
            }),
        },
    ];
    for token in invalid {
        let (status, code) = failure(server.client.create_token(&token).await);
        assert_eq!(code, "malformed_request", "{token:?}");
        assert!(status.is_client_error(), "{token:?}: {status}");
    }

    let unknown = uuid::Uuid::nil();
    assert_eq!(
        failure(server.client.revoke_token(unknown).await),
        (StatusCode::NOT_FOUND, "token_not_found".into())
    );
    let not_an_id = reqwest::Client::new()
        .post(server.url("/v1/tokens/nope/revoke"))
        .bearer_auth(&server.admin_token)
        .send()
        .await
        .unwrap();
    assert_eq!(not_an_id.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn bootstrapping_mints_the_first_admin_token_only_while_none_is_unrevoked(pool: PgPool) {
    let server = TestServer::without_tokens(pool).await;
    let client = server.anonymous();

    let wrong = client.bootstrap_token(&"x".repeat(40), "admin").await;
    assert_eq!(
        failure(wrong),
        (StatusCode::UNAUTHORIZED, "invalid_token".into())
    );
    let first = client
        .bootstrap_token(BOOTSTRAP_KEY, "admin")
        .await
        .unwrap();
    assert_eq!(first.token.scope, Scope::Admin);
    let admin = server.client_with(&first.value);
    admin.list_tokens().await.unwrap();

    let again = client.bootstrap_token(BOOTSTRAP_KEY, "second").await;
    assert_eq!(
        failure(again),
        (StatusCode::CONFLICT, "already_bootstrapped".into())
    );

    // Once every admin Token is revoked, the bootstrap key is the way back in.
    admin.revoke_token(first.token.id).await.unwrap();
    let recovered = client
        .bootstrap_token(BOOTSTRAP_KEY, "recovered")
        .await
        .unwrap();
    let mut names: Vec<_> = server
        .client_with(&recovered.value)
        .list_tokens()
        .await
        .unwrap()
        .into_iter()
        .map(|token| (token.name, token.revoked_at.is_some()))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [("admin".to_owned(), true), ("recovered".to_owned(), false)]
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn bootstrapping_is_disabled_without_a_bootstrap_key(pool: PgPool) {
    let clock = ManualClock::new(common::t0());
    let state = ApiState::new(pool, Arc::new(common::registry()), Arc::new(clock.clone()));
    let server = TestServer::serve(bia_server::router(state), clock).await;

    let result = server
        .anonymous()
        .bootstrap_token(BOOTSTRAP_KEY, "admin")
        .await;

    assert_eq!(
        failure(result),
        (StatusCode::FORBIDDEN, "bootstrap_disabled".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn every_route_but_the_probes_and_bootstrapping_needs_a_token(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let document = serde_json::to_value(bia_server::openapi()).unwrap();
    let http = reqwest::Client::new();

    let mut public = Vec::new();
    for (path, item) in document["paths"].as_object().unwrap() {
        for (method, operation) in item.as_object().unwrap() {
            let needs_token = operation["security"]
                .as_array()
                .is_some_and(|schemes| schemes.iter().any(|s| s.get("token").is_some()));
            if !needs_token {
                public.push(format!("{method} {path}"));
                continue;
            }
            let concrete = path.replace("{token_id}", &uuid::Uuid::nil().to_string());
            let response = http
                .request(
                    method.to_uppercase().parse().unwrap(),
                    server.url(&concrete),
                )
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
        }
    }
    public.sort();
    assert_eq!(
        public,
        ["get /health", "get /ready", "post /v1/tokens/bootstrap"]
    );
}
