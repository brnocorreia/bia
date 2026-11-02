//! The HTTP API harness: an in-process server on its own port, over the database
//! `#[sqlx::test]` made for the test, on a manual Clock, driven through `bia-client`.

#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use bia_client::{CreatedToken, NewToken, Scope, WebhookBinding};
use bia_core::action::flow::Flow;
use bia_core::action::queue::QueueAction;
use bia_core::action::testing::FakeAction;
use bia_core::action::{
    Action, ActionExample, ActionRegistry, CancellationToken, Effect, InMemoryRegistry,
};
use bia_core::clock::ManualClock;
use bia_server::{ApiConfig, ApiState, SetupCode, Wait};
use chrono::{DateTime, Utc};
use schemars::json_schema;
use serde_json::json;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub mod webhook;

/// The password every test server's User is set up with.
pub const PASSWORD: &str = "test-password-0123456789";

/// The `public_url` of every test server that does not set its own. Nothing listens there:
/// tests only send it as the `Origin` of requests made with a Session.
pub const PUBLIC_URL: &str = "http://bia.test";

/// The instant every test server's Clock starts at.
pub fn t0() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

/// Stops its server when dropped.
pub struct TestServer {
    /// Sends an admin Token, minted through the Session setup started, or no Token at all
    /// when the server was started [`without_tokens`](Self::without_tokens).
    pub client: bia_client::Client,
    /// The admin Token's value, empty without one.
    pub admin_token: String,
    /// The value of the Session cookie setup started, empty without one. See
    /// [`session_client`](Self::session_client).
    pub session: String,
    /// The origin the server accepts on requests made with a Session.
    pub origin: String,
    /// The code of this server's Setup Link, empty when its state was built elsewhere.
    pub setup_code: String,
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

    /// [`start`](Self::start) with the Actions of `registry` instead of [`registry()`].
    pub async fn start_with_registry(pool: PgPool, registry: Arc<InMemoryRegistry>) -> Self {
        let clock = ManualClock::new(t0());
        let (setup_code, code) = SetupCode::generate().unwrap();
        let config = ApiConfig {
            setup_code: Some(setup_code),
            public_url: Some(PUBLIC_URL.parse().unwrap()),
            ..ApiConfig::default()
        };
        let state = ApiState::new(pool, registry, Arc::new(clock.clone())).with_config(config);
        Self::set_up(bia_server::router(state), clock, code, PUBLIC_URL).await
    }

    /// [`start`](Self::start) with `config`, whose setup code is always a fresh one.
    pub async fn start_with(pool: PgPool, config: ApiConfig) -> Self {
        let clock = ManualClock::new(t0());
        let origin = config.public_url.as_ref().map_or_else(
            || PUBLIC_URL.to_owned(),
            |url| url.origin().ascii_serialization(),
        );
        let (state, code) = state(pool, &clock, config);
        Self::set_up(bia_server::router(state), clock, code, &origin).await
    }

    /// [`serve`](Self::serve), then sets up the User with `code` and [`PASSWORD`]. Setup starts
    /// a Session, which mints the admin Token `client` sends; `origin` is the server's
    /// `public_url`.
    pub async fn set_up(router: Router, clock: ManualClock, code: String, origin: &str) -> Self {
        let mut server = Self::serve(router, clock).await;
        server.origin = origin.to_owned();
        let done = server.anonymous().set_up(&code, PASSWORD).await.unwrap();
        server.session = done.session;
        let admin = server.mint(Scope::Admin, None).await;
        server.client = server.anonymous().with_token(&admin.value);
        server.admin_token = admin.value;
        server.setup_code = code;
        server
    }

    /// A server whose User is not set up yet, so its `client` sends no Token.
    pub async fn without_tokens(pool: PgPool) -> Self {
        let clock = ManualClock::new(t0());
        let (state, code) = state(pool, &clock, ApiConfig::default());
        let mut server = Self::serve(bia_server::router(state), clock).await;
        server.setup_code = code;
        server
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
            session: String::new(),
            origin: PUBLIC_URL.to_owned(),
            setup_code: String::new(),
            url,
            clock,
            shutdown,
            task,
        }
    }

    /// A client that sends the Session cookie setup started, and the `Origin` the server
    /// accepts with it, as the UI does.
    pub fn session_client(&self) -> bia_client::Client {
        self.client_with_session(&self.session)
    }

    /// A client that sends `session` as the Session cookie, and the accepted `Origin`.
    pub fn client_with_session(&self, session: &str) -> bia_client::Client {
        self.anonymous()
            .with_session(session)
            .with_origin(&self.origin)
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

    /// Minted with the Session when there is no admin Token yet.
    async fn mint(&self, scope: Scope, webhook: Option<WebhookBinding>) -> CreatedToken {
        let token = NewToken {
            name: format!("test-{scope:?}").to_lowercase(),
            scope,
            webhook,
        };
        let client = if self.admin_token.is_empty() {
            self.session_client()
        } else {
            self.client.clone()
        };
        client.create_token(&token).await.unwrap()
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

/// The state of a test server on `clock` with a fresh setup code, and that code.
pub fn state(pool: PgPool, clock: &ManualClock, config: ApiConfig) -> (ApiState, String) {
    let (setup_code, code) = SetupCode::generate().unwrap();
    let config = ApiConfig {
        setup_code: Some(setup_code),
        public_url: config
            .public_url
            .clone()
            .or_else(|| Some(PUBLIC_URL.parse().unwrap())),
        ..config
    };
    let state =
        ApiState::new(pool, Arc::new(registry()), Arc::new(clock.clone())).with_config(config);
    (state, code)
}

/// A [`Wait`] that returns at once and remembers what it was asked to wait for, so a test sees
/// the throttle's delays without sleeping.
#[derive(Debug, Default)]
pub struct RecordedWaits(std::sync::Mutex<Vec<std::time::Duration>>);

impl RecordedWaits {
    pub fn waits(&self) -> Vec<std::time::Duration> {
        self.0.lock().unwrap().clone()
    }
}

impl Wait for RecordedWaits {
    fn wait(
        &self,
        duration: std::time::Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        self.0.lock().unwrap().push(duration);
        Box::pin(std::future::ready(()))
    }
}

/// The flow and queue Actions, `test.echo` with schemas and an example, and a disabled
/// dangerous side-effecting `test.shell`.
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
    let queue = QueueAction::all()
        .into_iter()
        .map(|action| Arc::new(action) as Arc<dyn Action>);
    for action in actions.into_iter().chain(flow).chain(queue) {
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

/// A Worker running Jobs from the database on `clock`, stopped when dropped.
pub struct Workers {
    shutdown: CancellationToken,
    handle: JoinHandle<()>,
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.handle.abort();
    }
}

/// Starts a Worker over `registry`. Engine time only moves when a test moves `clock`.
pub fn spawn_worker(
    pool: &PgPool,
    clock: &ManualClock,
    registry: Arc<InMemoryRegistry>,
) -> Workers {
    let interpreter = bia_engine::Interpreter::new(pool.clone(), registry, Arc::new(clock.clone()));
    let worker = bia_engine::Worker::new(
        "test-worker",
        interpreter,
        bia_engine::WorkerConfig::default(),
    );
    let shutdown = CancellationToken::new();
    let token = shutdown.clone();
    let handle = tokio::spawn(async move { worker.run(token).await.unwrap() });
    Workers { shutdown, handle }
}

/// Waits for other tasks to get somewhere, which only a test moving the Clock can hurry.
pub async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let waited = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while !check().await {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "timed out waiting for {what}");
}
