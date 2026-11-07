//! Passkeys: registered through a WebAuthn ceremony, used to sign in instead of the password,
//! and removed only by a Session that signed in recently. A software authenticator plays the
//! browser's part.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use bia_client::{Ceremony, Client, Error, Origin, Passkey, Scope};
use bia_core::clock::Clock;
use bia_server::ApiConfig;
use chrono::TimeDelta;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use webauthn_authenticator_rs::AuthenticatorBackend;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

mod common;

use common::{PUBLIC_URL, RecordedWaits, TestServer};

fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> (StatusCode, String) {
    match result {
        Err(Error::Api { status, body }) => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    }
}

fn refused() -> (StatusCode, String) {
    (StatusCode::UNAUTHORIZED, "invalid_credentials".into())
}

fn invalid_ceremony() -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, "invalid_ceremony".into())
}

/// The test origins are plain HTTP on a made-up host, which a real browser would refuse, so
/// the authenticator is driven below the browser's own checks.
const TIMEOUT_MS: u32 = 60_000;

/// A software authenticator that claims user verification, which passkeys require and it cannot
/// perform.
///
/// What a browser does with the authenticator: answers the options of a ceremony, as seen from
/// `origin`.
struct Browser(SoftPasskey);

impl Browser {
    fn new() -> Self {
        Self(SoftPasskey::new(true))
    }

    fn create(&mut self, origin: &str, ceremony: &Ceremony) -> Value {
        let options: CreationChallengeResponse =
            serde_json::from_value(ceremony.options.clone()).unwrap();
        let credential = self
            .0
            .perform_register(Url::parse(origin).unwrap(), options.public_key, TIMEOUT_MS)
            .unwrap();
        serde_json::to_value(credential).unwrap()
    }

    fn get(&mut self, origin: &str, ceremony: &Ceremony) -> Value {
        let options: RequestChallengeResponse =
            serde_json::from_value(ceremony.options.clone()).unwrap();
        let credential = self
            .0
            .perform_auth(Url::parse(origin).unwrap(), options.public_key, TIMEOUT_MS)
            .unwrap();
        serde_json::to_value(credential).unwrap()
    }
}

async fn register(
    server: &TestServer,
    browser: &mut Browser,
    client: &Client,
    name: &str,
) -> Passkey {
    let ceremony = client.begin_passkey_registration(name).await.unwrap();
    let credential = browser.create(&server.origin, &ceremony);
    client
        .finish_passkey_registration(ceremony.ceremony_id, &credential)
        .await
        .unwrap()
}

async fn sign_in_with_passkey(
    server: &TestServer,
    browser: &mut Browser,
) -> Result<bia_client::SignedIn, Error> {
    let anonymous = server.anonymous();
    let ceremony = anonymous.begin_passkey_sign_in().await?;
    let credential = browser.get(&server.origin, &ceremony);
    anonymous
        .finish_passkey_sign_in(ceremony.ceremony_id, &credential)
        .await
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_registered_passkey_signs_in_after_signing_out(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let session = server.session_client();
    let mut browser = Browser::new();

    let passkey = register(&server, &mut browser, &session, "laptop").await;
    session.sign_out().await.unwrap();
    assert!(session.current_token().await.is_err());
    server.clock.advance(TimeDelta::minutes(3));
    let signed_in = sign_in_with_passkey(&server, &mut browser).await.unwrap();

    assert_eq!(passkey.name, "laptop");
    assert_eq!(signed_in.token.origin, Origin::Session);
    assert_eq!(signed_in.token.scope, Scope::Admin);
    let again = server.client_with_session(&signed_in.session);
    assert_eq!(again.current_token().await.unwrap().id, signed_in.token.id);
    let listed = again.list_passkeys().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, passkey.id);
    assert_eq!(listed[0].last_used_at, Some(server.clock.now()));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_passkey_can_be_renamed_and_the_list_names_each_one(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let session = server.session_client();
    let mut laptop = Browser::new();
    let mut phone = Browser::new();
    let first = register(&server, &mut laptop, &session, "laptop").await;
    server.clock.advance(TimeDelta::minutes(1));
    register(&server, &mut phone, &session, "phone").await;

    let renamed = session
        .rename_passkey(first.id, "work laptop")
        .await
        .unwrap();

    assert_eq!(renamed.name, "work laptop");
    let names: Vec<_> = session
        .list_passkeys()
        .await
        .unwrap()
        .into_iter()
        .map(|passkey| passkey.name)
        .collect();
    assert_eq!(names, ["work laptop", "phone"]);
    assert_eq!(
        failure(session.rename_passkey(Uuid::nil(), "x").await),
        (StatusCode::NOT_FOUND, "passkey_not_found".into())
    );
    assert_eq!(
        failure(session.rename_passkey(first.id, "  ").await),
        (StatusCode::BAD_REQUEST, "malformed_request".into())
    );
    // Both authenticators sign in.
    sign_in_with_passkey(&server, &mut laptop).await.unwrap();
    sign_in_with_passkey(&server, &mut phone).await.unwrap();
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn removing_a_passkey_needs_a_session_that_signed_in_within_ten_minutes(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let session = server.session_client();
    let mut browser = Browser::new();
    let passkey = register(&server, &mut browser, &session, "laptop").await;
    let reauthentication = (StatusCode::FORBIDDEN, "reauthentication_required".into());

    // A Token is no Session, so it cannot show a recent sign-in.
    assert_eq!(
        failure(server.client.delete_passkey(passkey.id).await),
        reauthentication
    );
    server.clock.advance(TimeDelta::minutes(10));
    assert_eq!(
        failure(session.delete_passkey(passkey.id).await),
        reauthentication
    );
    assert_eq!(session.list_passkeys().await.unwrap().len(), 1);

    let fresh = server.anonymous().sign_in(common::PASSWORD).await.unwrap();
    let fresh = server.client_with_session(&fresh.session);
    server.clock.advance(TimeDelta::minutes(9));
    fresh.delete_passkey(passkey.id).await.unwrap();

    assert!(fresh.list_passkeys().await.unwrap().is_empty());
    assert_eq!(
        failure(fresh.delete_passkey(passkey.id).await),
        (StatusCode::NOT_FOUND, "passkey_not_found".into())
    );
    assert_eq!(
        failure(sign_in_with_passkey(&server, &mut browser).await),
        refused()
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_registration_from_another_origin_is_refused(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let session = server.session_client();
    let ceremony = session.begin_passkey_registration("laptop").await.unwrap();

    let credential = Browser::new().create("http://evil.test", &ceremony);

    assert_eq!(
        failure(
            session
                .finish_passkey_registration(ceremony.ceremony_id, &credential)
                .await
        ),
        invalid_ceremony()
    );
    assert!(session.list_passkeys().await.unwrap().is_empty());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_registration_for_another_rp_id_is_refused(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let session = server.session_client();
    let mut ceremony = session.begin_passkey_registration("laptop").await.unwrap();
    // An attacker's page asks the authenticator for a passkey under its own RP ID.
    ceremony.options["publicKey"]["rp"]["id"] = json!("evil.test");

    let credential = Browser::new().create("http://evil.test", &ceremony);

    assert_eq!(
        failure(
            session
                .finish_passkey_registration(ceremony.ceremony_id, &credential)
                .await
        ),
        invalid_ceremony()
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_sign_in_response_from_another_origin_or_rp_id_is_refused(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let mut browser = Browser::new();
    register(&server, &mut browser, &server.session_client(), "laptop").await;
    let anonymous = server.anonymous();

    let ceremony = anonymous.begin_passkey_sign_in().await.unwrap();
    let from_elsewhere = browser.get(PUBLIC_URL, &ceremony);
    let mut forged = from_elsewhere.clone();
    // The response claims to come from a page that is not the server's.
    forged["response"]["clientDataJSON"] = Value::String(client_data_for(
        &from_elsewhere["response"]["clientDataJSON"],
        "http://evil.test",
    ));
    assert_eq!(
        failure(
            anonymous
                .finish_passkey_sign_in(ceremony.ceremony_id, &forged)
                .await
        ),
        refused()
    );

    // The ceremony is spent by the failure; a new one signs in with the same authenticator.
    sign_in_with_passkey(&server, &mut browser).await.unwrap();
}

/// `client_data_json`, a base64url string, with its `origin` replaced.
fn client_data_for(client_data_json: &Value, origin: &str) -> String {
    use base64::Engine;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let raw = engine.decode(client_data_json.as_str().unwrap()).unwrap();
    let mut data: Value = serde_json::from_slice(&raw).unwrap();
    data["origin"] = json!(origin);
    engine.encode(serde_json::to_vec(&data).unwrap())
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_ceremony_expires_after_five_minutes_and_works_once(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let session = server.session_client();
    let mut browser = Browser::new();

    let slow = session.begin_passkey_registration("slow").await.unwrap();
    let credential = browser.create(&server.origin, &slow);
    server.clock.advance(TimeDelta::minutes(5));
    assert_eq!(
        failure(
            session
                .finish_passkey_registration(slow.ceremony_id, &credential)
                .await
        ),
        invalid_ceremony()
    );

    let prompt = session.begin_passkey_registration("prompt").await.unwrap();
    let credential = browser.create(&server.origin, &prompt);
    server.clock.advance(TimeDelta::minutes(4));
    session
        .finish_passkey_registration(prompt.ceremony_id, &credential)
        .await
        .unwrap();
    assert_eq!(
        failure(
            session
                .finish_passkey_registration(prompt.ceremony_id, &credential)
                .await
        ),
        invalid_ceremony()
    );

    let ceremony = server.anonymous().begin_passkey_sign_in().await.unwrap();
    let credential = browser.get(&server.origin, &ceremony);
    server.clock.advance(TimeDelta::minutes(6));
    assert_eq!(
        failure(
            server
                .anonymous()
                .finish_passkey_sign_in(ceremony.ceremony_id, &credential)
                .await
        ),
        refused()
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn failed_passkey_sign_ins_count_towards_the_throttle(pool: PgPool) {
    let waits = Arc::new(RecordedWaits::default());
    let config = ApiConfig {
        sign_in_wait: Some(waits.clone()),
        ..ApiConfig::default()
    };
    let server = TestServer::start_with(pool, config).await;
    let mut browser = Browser::new();
    register(&server, &mut browser, &server.session_client(), "laptop").await;
    let anonymous = server.anonymous();
    let garbage = json!({ "id": "x" });

    for _ in 0..5 {
        anonymous
            .finish_passkey_sign_in(Uuid::new_v4(), &garbage)
            .await
            .unwrap_err();
    }
    assert!(waits.waits().is_empty());
    anonymous
        .finish_passkey_sign_in(Uuid::new_v4(), &garbage)
        .await
        .unwrap_err();
    // Passkey failures and password failures share one count.
    anonymous
        .sign_in("not the password at all")
        .await
        .unwrap_err();
    sign_in_with_passkey(&server, &mut browser).await.unwrap();

    let seconds = Duration::from_secs;
    assert_eq!(waits.waits(), [seconds(1), seconds(2), seconds(4)]);
    // The success cleared the count.
    anonymous
        .finish_passkey_sign_in(Uuid::new_v4(), &garbage)
        .await
        .unwrap_err();
    assert_eq!(waits.waits().len(), 3);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn signing_in_with_a_passkey_needs_one_registered(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let result = server.anonymous().begin_passkey_sign_in().await;

    assert_eq!(failure(result), refused());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn passkey_routes_need_an_admin(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let agent = server.client_with(&server.token(Scope::Agent).await);

    assert_eq!(
        failure(agent.list_passkeys().await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );
    assert_eq!(
        failure(server.anonymous().list_passkeys().await),
        (StatusCode::UNAUTHORIZED, "missing_token".into())
    );
    assert_eq!(
        failure(agent.begin_passkey_registration("x").await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn passkeys_are_unavailable_when_the_public_url_host_is_an_ip_address(pool: PgPool) {
    let config = ApiConfig {
        public_url: Some("http://127.0.0.1:8080".parse().unwrap()),
        ..ApiConfig::default()
    };
    let server = TestServer::start_with(pool, config).await;

    let result = server
        .session_client()
        .begin_passkey_registration("laptop")
        .await;

    assert_eq!(
        failure(result),
        (StatusCode::CONFLICT, "passkeys_unavailable".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_ceremony_on_the_ui_origin_works_and_another_port_of_the_host_does_not(pool: PgPool) {
    let config = ApiConfig {
        public_url: Some("http://bia.test:8080".parse().unwrap()),
        ui_url: Some("http://bia.test:5173".parse().unwrap()),
        ..ApiConfig::default()
    };
    let server = TestServer::start_with(pool, config).await;
    let session = server.session_client();

    let ceremony = session.begin_passkey_registration("laptop").await.unwrap();
    let elsewhere = Browser::new().create("http://bia.test:9999", &ceremony);
    assert_eq!(
        failure(
            session
                .finish_passkey_registration(ceremony.ceremony_id, &elsewhere)
                .await
        ),
        invalid_ceremony()
    );

    let ceremony = session.begin_passkey_registration("laptop").await.unwrap();
    let from_ui = Browser::new().create("http://bia.test:5173", &ceremony);
    session
        .finish_passkey_registration(ceremony.ceremony_id, &from_ui)
        .await
        .unwrap();
}
