//! A Workflow with webhook Triggers, and requests to them.

use bia_server::ApiConfig;
use bia_store::secret::MasterKey;
use hmac::{KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use super::TestServer;

pub const WORKFLOW: &str = "signup";
/// Needs a Token.
pub const PLAIN: &str = "hook";
/// Needs a signature under [`HMAC_KEY`] in [`SIGNATURE_HEADER`].
pub const SIGNED: &str = "signed";
pub const SIGNATURE_HEADER: &str = "X-Hub-Signature-256";
pub const HMAC_KEY: &str = "whsec-0123456789-not-a-real-key";

/// With a master key, so the HMAC Secret can be stored.
pub fn config() -> ApiConfig {
    ApiConfig {
        master_key: Some(MasterKey::parse(&"ab".repeat(32)).unwrap()),
        ..ApiConfig::default()
    }
}

/// Webhook Triggers [`PLAIN`] and [`SIGNED`], a manual and a cron Trigger, an `input_schema`
/// requiring `email`, and one Step echoing `trigger`.
pub fn definition() -> Value {
    json!({
        "schema_version": 1,
        "id": WORKFLOW,
        "triggers": [
            { "id": PLAIN, "type": "webhook" },
            {
                "id": SIGNED,
                "type": "webhook",
                "hmac": { "header": SIGNATURE_HEADER, "secret": "secrets.SIGNUP_HMAC" },
            },
            { "id": "by_hand", "type": "manual" },
            { "id": "nightly", "type": "cron", "expr": "0 3 * * *" },
        ],
        "input_schema": {
            "type": "object",
            "required": ["email"],
            "properties": { "email": { "type": "string" } },
        },
        "steps": [
            { "id": "echo", "action": "test.echo", "params": { "greeting": "hi", "trigger": "{{ trigger }}" } },
        ],
    })
}

/// Publishes and activates `definition` as the next Version, and stores the HMAC Secret.
pub async fn deploy(server: &TestServer, definition: &Value) -> i32 {
    let admin = &server.client;
    let id = definition["id"].as_str().unwrap();
    let draft = admin.create_draft(id, definition).await.unwrap();
    let number = draft.version.summary.number;
    admin.publish_version(id, number).await.unwrap();
    admin.activate_version(id, number).await.unwrap();
    admin.put_secret("SIGNUP_HMAC", HMAC_KEY).await.unwrap();
    number
}

/// `sha256=` and the lowercase hex HMAC-SHA256 of `body` under [`HMAC_KEY`].
pub fn sign(body: &[u8]) -> String {
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(HMAC_KEY.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// A POST of `body` to `/hooks/{workflow}/{trigger}`.
pub fn post(
    server: &TestServer,
    workflow: &str,
    trigger: &str,
    body: impl Into<reqwest::Body>,
) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(server.url(&format!("/hooks/{workflow}/{trigger}")))
        .header("content-type", "application/json")
        .body(body)
}
