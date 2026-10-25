//! The HTTP API harness: an in-process server on its own port, over the database
//! `#[sqlx::test]` made for the test, on a manual Clock, driven through `bia-client`.

#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use bia_client::{CreatedToken, NewToken, Scope, WebhookBinding};
use bia_core::action::flow::Flow;
use bia_core::action::testing::FakeAction;
use bia_core::action::{
    Action, ActionExample, ActionRegistry, CancellationToken, Effect, InMemoryRegistry,
};
use bia_core::clock::ManualClock;
use bia_server::{ApiConfig, ApiState, BootstrapKey};
use chrono::{DateTime, Utc};
use schemars::json_schema;
use serde_json::json;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// The bootstrap key every test server is started with.
pub const BOOTSTRAP_KEY: &str = "test-bootstrap-key-0123456789abcdef";

/// The instant every test server's Clock starts at.
pub fn t0() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

/// Stops its server when dropped.
pub struct TestServer {
    /// Sends the admin Token bootstrapped at start, or no Token at all when the server was
    /// started [`without_tokens`](Self::without_tokens).
    pub client: bia_client::Client,
    /// The admin Token's value, empty without one.
    pub admin_token: String,
    /// Where the server listens, e.g. `http://127.0.0.1:53124`.
    pub url: String,
    pub clock: ManualClock,
    shutdown: CancellationToken,
    task: JoinHandle<std::io::Result<()>>,
}

impl TestServer {
    /// A server whose `client` holds an admin Token.
    pub async fn start(pool: PgPool) -> Self {
        Self::start_with(pool, ApiConfig::default()).await
    }

    /// [`start`](Self::start) with `config`, whose bootstrap key is always [`BOOTSTRAP_KEY`].
    pub async fn start_with(pool: PgPool, config: ApiConfig) -> Self {
        let clock = ManualClock::new(t0());
        let router = bia_server::router(state(pool, &clock, config));
        Self::bootstrapped(router, clock).await
    }

    /// [`serve`](Self::serve), then bootstraps the admin Token `client` sends.
    pub async fn bootstrapped(router: Router, clock: ManualClock) -> Self {
        let mut server = Self::serve(router, clock).await;
        let admin = server
            .client
            .bootstrap_token(BOOTSTRAP_KEY, "test-admin")
            .await
            .unwrap();
        server.client = server.anonymous().with_token(&admin.value);
        server.admin_token = admin.value;
        server
    }

    /// A server no Token was bootstrapped on, whose `client` sends none.
    pub async fn without_tokens(pool: PgPool) -> Self {
        let clock = ManualClock::new(t0());
        Self::serve(
            bia_server::router(state(pool, &clock, ApiConfig::default())),
            clock,
        )
        .await
    }

    /// Serves `router`, e.g. [`bia_server::router`] with routes a test adds, whose state was
    /// built on `clock`.
    pub async fn serve(router: Router, clock: ManualClock) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(bia_server::serve(listener, router, shutdown.clone()));
        Self {
            client: bia_client::Client::new(&url).unwrap(),
            admin_token: String::new(),
            url,
            clock,
            shutdown,
            task,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.url)
    }

    /// A client that sends no Token.
    pub fn anonymous(&self) -> bia_client::Client {
        bia_client::Client::new(&self.url).unwrap()
    }

    /// The value of a new Token with `scope`, created with the admin Token.
    pub async fn token(&self, scope: Scope) -> String {
        self.mint(scope, None).await.value
    }

    /// A new Token with `scope`, created with the admin Token.
    pub async fn created_token(&self, scope: Scope) -> CreatedToken {
        self.mint(scope, None).await
    }

    /// The value of a new `webhook` Token bound to `workflow_id` and `trigger_id`.
    pub async fn webhook_token(&self, workflow_id: &str, trigger_id: &str) -> String {
        let binding = WebhookBinding {
            workflow_id: workflow_id.to_owned(),
            trigger_id: trigger_id.to_owned(),
        };
        self.mint(Scope::Webhook, Some(binding)).await.value
    }

    async fn mint(&self, scope: Scope, webhook: Option<WebhookBinding>) -> CreatedToken {
        let token = NewToken {
            name: format!("test-{scope:?}").to_lowercase(),
            scope,
            webhook,
        };
        self.client.create_token(&token).await.unwrap()
    }

    /// A client that sends `token`.
    pub fn client_with(&self, token: &str) -> bia_client::Client {
        self.anonymous().with_token(token)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.task.abort();
    }
}

/// The state of a test server on `clock`, with [`BOOTSTRAP_KEY`].
pub fn state(pool: PgPool, clock: &ManualClock, config: ApiConfig) -> ApiState {
    let config = ApiConfig {
        bootstrap_key: Some(BootstrapKey::new(BOOTSTRAP_KEY).unwrap()),
        ..config
    };
    ApiState::new(pool, Arc::new(registry()), Arc::new(clock.clone())).with_config(config)
}

/// The flow Actions, `test.echo` with schemas and an example, and a disabled dangerous
/// side-effecting `test.shell`.
pub fn registry() -> InMemoryRegistry {
    let mut registry = InMemoryRegistry::new();
    let echo = FakeAction::new("test.echo").with_descriptor(|d| {
        d.description = "Returns its Params.".into();
        d.param_schema = json_schema!({
            "type": "object",
            "properties": { "greeting": { "type": "string" } },
            "required": ["greeting"],
        });
        d.output_schema = json_schema!({ "type": "object" });
        d.examples.push(ActionExample::new(
            "Echo a greeting",
            json!({ "greeting": "hi" }).as_object().unwrap().clone(),
            json!({ "greeting": "hi" }),
        ));
    });
    let shell = FakeAction::new("test.shell").with_descriptor(|d| {
        d.dangerous = true;
        d.effect = Effect::SideEffecting;
    });
    let actions: Vec<Arc<dyn Action>> = vec![Arc::new(echo), Arc::new(shell)];
    let flow = Flow::all()
        .into_iter()
        .map(|action| Arc::new(action) as Arc<dyn Action>);
    for action in actions.into_iter().chain(flow) {
        registry.register(action).unwrap();
    }
    registry
}

pub fn catalogue_names() -> Vec<String> {
    registry()
        .catalogue()
        .into_iter()
        .map(|entry| entry.descriptor.name.as_str().to_owned())
        .collect()
}
