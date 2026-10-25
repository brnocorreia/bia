//! The HTTP API harness: an in-process server on its own port, over the database
//! `#[sqlx::test]` made for the test, driven through `bia-client`.

#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use bia_core::action::flow::Flow;
use bia_core::action::testing::FakeAction;
use bia_core::action::{
    Action, ActionExample, ActionRegistry, CancellationToken, Effect, InMemoryRegistry,
};
use bia_server::ApiState;
use schemars::json_schema;
use serde_json::json;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Stops its server when dropped.
pub struct TestServer {
    pub client: bia_client::Client,
    /// Where the server listens, e.g. `http://127.0.0.1:53124`.
    pub url: String,
    shutdown: CancellationToken,
    task: JoinHandle<std::io::Result<()>>,
}

impl TestServer {
    pub async fn start(pool: PgPool) -> Self {
        Self::serve(bia_server::router(state(pool))).await
    }

    /// Serves `router`, e.g. [`bia_server::router`] with routes a test adds.
    pub async fn serve(router: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(bia_server::serve(listener, router, shutdown.clone()));
        Self {
            client: bia_client::Client::new(&url).unwrap(),
            url,
            shutdown,
            task,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.url)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.task.abort();
    }
}

pub fn state(pool: PgPool) -> ApiState {
    ApiState::new(pool, Arc::new(registry()))
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
