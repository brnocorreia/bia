//! The MCP server (ADR-0048): each tool calls its endpoint and returns the endpoint's answer,
//! and Scopes are the API's. Driven through a real MCP client over streamable HTTP.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use bia_client::{RunStatus, Scope};
use bia_core::action::testing::FakeAction;
use bia_core::action::{Action, ActionError, InMemoryRegistry};
use reqwest::StatusCode;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

mod common;

use common::{TestServer, eventually, spawn_worker};

type Client = RunningService<rmcp::RoleClient, ()>;

async fn connect(server: &TestServer, token: &str) -> Client {
    let config = StreamableHttpClientTransportConfig::with_uri(server.url("/mcp"))
        .auth_header(token.to_owned());
    ().serve(StreamableHttpClientTransport::from_config(config))
        .await
        .unwrap()
}

async fn call(mcp: &Client, tool: &str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object")
    };
    mcp.call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap()
}

/// The structured answer of a tool that succeeded.
async fn ok(mcp: &Client, tool: &str, arguments: Value) -> Value {
    let result = call(mcp, tool, arguments).await;
    assert_ne!(result.is_error, Some(true), "{tool} failed: {result:?}");
    result.structured_content.unwrap()
}

/// The error body of a tool that failed.
async fn err(mcp: &Client, tool: &str, arguments: Value) -> Value {
    let result = call(mcp, tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
    result.structured_content.unwrap()
}

struct Fixture {
    server: TestServer,
    mcp: Client,
    _workers: common::Workers,
    http: reqwest::Client,
}

async fn fixture(pool: PgPool) -> Fixture {
    let mut registry = InMemoryRegistry::new();
    let echo = FakeAction::new("test.echo").returning(json!({ "ok": true }));
    let fail = FakeAction::new("test.fail").failing(ActionError::fatal("always fails"));
    let wait = FakeAction::new("test.wait").until_cancelled();
    for action in [echo, fail, wait] {
        registry
            .register(Arc::new(action) as Arc<dyn Action>)
            .unwrap();
    }
    let registry = Arc::new(registry);
    let server = TestServer::start_with_registry(pool.clone(), registry.clone()).await;
    let workers = spawn_worker(&pool, &server.clock, registry);
    let agent = server.token(Scope::Agent).await;
    let mcp = connect(&server, &agent).await;
    Fixture {
        mcp,
        _workers: workers,
        http: reqwest::Client::new(),
        server,
    }
}

fn definition(id: &str, action: &str) -> Value {
    json!({
        "schema_version": 1,
        "id": id,
        "triggers": [{ "id": "by_hand", "type": "manual" }],
        "steps": [{ "id": "only", "action": action, "params": {} }],
    })
}

impl Fixture {
    /// What the API answers the admin Token for `GET path`.
    async fn api(&self, path: &str) -> Value {
        self.http
            .get(self.server.url(path))
            .bearer_auth(&self.server.admin_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    /// What the API answers the admin Token for `POST path` with `body`.
    async fn api_post(&self, path: &str, body: &Value) -> Value {
        self.http
            .post(self.server.url(path))
            .bearer_auth(&self.server.admin_token)
            .json(body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    /// Publishes and activates a Version of `definition` with the admin Token.
    async fn deploy(&self, definition: &Value) -> i32 {
        let id = definition["id"].as_str().unwrap();
        let admin = &self.server.client;
        let number = admin
            .create_draft(id, definition)
            .await
            .unwrap()
            .version
            .summary
            .number;
        admin.publish_version(id, number).await.unwrap();
        admin.activate_version(id, number).await.unwrap();
        number
    }

    async fn status(&self, run: Uuid) -> RunStatus {
        self.server
            .client
            .get_run(run)
            .await
            .unwrap()
            .summary
            .status
    }

    async fn finished(&self, run: Uuid) -> RunStatus {
        eventually("the Run to finish", || async {
            self.status(run).await.is_finished()
        })
        .await;
        self.status(run).await
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_tool_list_and_its_descriptions_are_stable(pool: PgPool) {
    let f = fixture(pool).await;
    let tools = f.mcp.list_all_tools().await.unwrap();
    insta::assert_json_snapshot!(tools);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn get_schema_and_list_actions_return_the_schema_endpoint(pool: PgPool) {
    let f = fixture(pool).await;
    let schema = f.api("/v1/schema").await;
    assert_eq!(ok(&f.mcp, "get_schema", json!({})).await, schema);
    assert_eq!(
        ok(&f.mcp, "list_actions", json!({})).await,
        json!({ "actions": schema["actions"] })
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn workflow_and_version_reads_return_their_endpoints(pool: PgPool) {
    let f = fixture(pool).await;
    f.deploy(&definition("greeter", "test.echo")).await;
    let mcp = &f.mcp;

    assert_eq!(
        ok(mcp, "list_workflows", json!({})).await,
        f.api("/v1/workflows").await
    );
    assert_eq!(
        ok(mcp, "get_workflow", json!({ "workflow_id": "greeter" })).await,
        f.api("/v1/workflows/greeter").await
    );
    assert_eq!(
        ok(mcp, "list_versions", json!({ "workflow_id": "greeter" })).await,
        f.api("/v1/workflows/greeter/versions").await
    );
    assert_eq!(
        ok(
            mcp,
            "get_version",
            json!({ "workflow_id": "greeter", "number": 1 })
        )
        .await,
        f.api("/v1/workflows/greeter/versions/1").await
    );
    let missing = err(mcp, "get_workflow", json!({ "workflow_id": "nope" })).await;
    assert_eq!(missing["code"], "workflow_not_found");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn validate_workflow_returns_diagnostics_with_path_code_and_suggestion(pool: PgPool) {
    let f = fixture(pool).await;
    let mut broken = definition("broken", "test.ech");
    broken["steps"][0]["action"] = json!("test.ech");

    let report = ok(&f.mcp, "validate_workflow", json!({ "definition": broken })).await;
    let direct = f.api_post("/v1/validate", &broken).await;
    assert_eq!(report, direct);
    assert_eq!(report["valid"], false);
    let diagnostic = &report["diagnostics"][0];
    assert!(diagnostic["path"].is_string());
    assert!(diagnostic["code"].is_string());
    assert!(diagnostic["suggestion"].is_string());

    f.server
        .client
        .create_draft("broken", &broken)
        .await
        .unwrap();
    let stored = ok(
        &f.mcp,
        "validate_workflow",
        json!({ "workflow_id": "broken", "number": 1, "target": "publish" }),
    )
    .await;
    assert_eq!(
        stored,
        f.api_post(
            "/v1/workflows/broken/versions/1/validate?target=publish",
            &json!(null)
        )
        .await
    );

    let both = err(
        &f.mcp,
        "validate_workflow",
        json!({ "definition": broken, "workflow_id": "broken", "number": 1 }),
    )
    .await;
    assert_eq!(both["code"], "malformed_request");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn create_draft_and_replace_draft_save_versions(pool: PgPool) {
    let f = fixture(pool).await;
    let saved = ok(
        &f.mcp,
        "create_draft",
        json!({ "workflow_id": "greeter", "definition": definition("greeter", "test.echo") }),
    )
    .await;
    assert_eq!(saved["version"]["number"], 1);
    assert_eq!(saved["version"]["state"], "draft");

    let replaced = ok(
        &f.mcp,
        "replace_draft",
        json!({
            "workflow_id": "greeter",
            "number": 1,
            "definition": definition("greeter", "test.fail"),
        }),
    )
    .await;
    assert_eq!(replaced["version"]["number"], 1);
    let stored = f.server.client.get_version("greeter", 1).await.unwrap();
    assert_eq!(stored.definition["steps"][0]["action"], "test.fail");

    let mismatch = err(
        &f.mcp,
        "create_draft",
        json!({ "workflow_id": "other", "definition": definition("greeter", "test.echo") }),
    )
    .await;
    assert_eq!(mismatch["code"], "workflow_id_mismatch");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn dry_run_maps_to_the_version_and_definition_endpoints(pool: PgPool) {
    let f = fixture(pool).await;
    f.server
        .client
        .create_draft("greeter", &definition("greeter", "test.echo"))
        .await
        .unwrap();

    let by_version = ok(
        &f.mcp,
        "dry_run",
        json!({ "workflow_id": "greeter", "number": 1, "mocks": { "only": { "output": 7 } } }),
    )
    .await;
    assert_eq!(
        by_version,
        f.api_post(
            "/v1/workflows/greeter/versions/1/dry-run",
            &json!({ "mocks": { "only": { "output": 7 } } })
        )
        .await
    );
    assert_eq!(by_version["dry_run"], true);
    assert_eq!(by_version["status"], "succeeded");

    let inline = definition("scratch", "test.echo");
    let by_definition = ok(&f.mcp, "dry_run", json!({ "definition": inline })).await;
    assert_eq!(
        by_definition,
        f.api_post("/v1/dry-run", &json!({ "definition": inline }))
            .await
    );
    assert_eq!(f.server.client.list_workflows().await.unwrap().len(), 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn diff_versions_returns_the_diff_endpoint(pool: PgPool) {
    let f = fixture(pool).await;
    for action in ["test.echo", "test.fail"] {
        f.server
            .client
            .create_draft("greeter", &definition("greeter", action))
            .await
            .unwrap();
    }
    let diff = ok(
        &f.mcp,
        "diff_versions",
        json!({ "workflow_id": "greeter", "from": 1, "to": 2 }),
    )
    .await;
    assert_eq!(diff, f.api("/v1/workflows/greeter/diff?from=1&to=2").await);
    assert!(!diff.to_string().is_empty());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn run_tools_start_read_list_cancel_retry_and_resume_runs(pool: PgPool) {
    let f = fixture(pool).await;
    f.deploy(&definition("greeter", "test.echo")).await;
    f.deploy(&definition("fragile", "test.fail")).await;
    let mcp = &f.mcp;

    let started = ok(
        mcp,
        "trigger_run",
        json!({ "workflow_id": "greeter", "input": { "who": "ana" } }),
    )
    .await;
    assert_eq!(started["workflow_id"], "greeter");
    let run_id: Uuid = started["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(f.finished(run_id).await, RunStatus::Succeeded);

    let run = ok(mcp, "get_run", json!({ "run_id": run_id })).await;
    assert_eq!(run, f.api(&format!("/v1/runs/{run_id}")).await);
    assert_eq!(run["input"], json!({ "who": "ana" }));

    let listed = ok(
        mcp,
        "list_runs",
        json!({ "workflow_id": "greeter", "status": "succeeded", "limit": 5 }),
    )
    .await;
    assert_eq!(
        listed,
        f.api("/v1/runs?workflow_id=greeter&status=succeeded&limit=5")
            .await
    );
    assert_eq!(listed["runs"].as_array().unwrap().len(), 1);

    let broken = ok(mcp, "trigger_run", json!({ "workflow_id": "fragile" })).await;
    let broken_id: Uuid = broken["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(f.finished(broken_id).await, RunStatus::Failed);
    let failures = ok(mcp, "list_failures", json!({})).await;
    assert_eq!(failures, f.api("/v1/runs/failures").await);
    assert_eq!(failures["runs"].as_array().unwrap().len(), 1);

    let retried = ok(mcp, "retry_run", json!({ "run_id": broken_id })).await;
    assert_ne!(retried["id"], json!(broken_id));
    assert_eq!(retried["workflow_id"], "fragile");
    let retried_id: Uuid = retried["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(f.finished(retried_id).await, RunStatus::Failed);

    let resumed = ok(mcp, "resume_run", json!({ "run_id": broken_id })).await;
    assert_eq!(resumed["id"], json!(broken_id));

    let late = err(mcp, "cancel_run", json!({ "run_id": run_id })).await;
    assert_eq!(
        late,
        f.api_post(&format!("/v1/runs/{run_id}/cancel"), &json!({}))
            .await
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn cancel_run_cancels_a_run_in_flight(pool: PgPool) {
    let f = fixture(pool).await;
    f.deploy(&definition("slow", "test.wait")).await;
    let started = f
        .server
        .client
        .trigger_run("slow", &json!({}), None)
        .await
        .unwrap();
    let requested = ok(&f.mcp, "cancel_run", json!({ "run_id": started.id })).await;
    assert_eq!(requested["id"], json!(started.id));
    assert_eq!(f.finished(started.id).await, RunStatus::Cancelled);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_bad_argument_is_a_tool_error_that_names_it(pool: PgPool) {
    let f = fixture(pool).await;
    let missing = err(&f.mcp, "get_run", json!({})).await;
    assert_eq!(missing["code"], "malformed_request");
    assert!(missing["message"].as_str().unwrap().contains("run_id"));
    let traversal = err(
        &f.mcp,
        "get_workflow",
        json!({ "workflow_id": "../tokens" }),
    )
    .await;
    assert_eq!(traversal["code"], "malformed_request");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_agent_token_has_no_tool_to_publish_or_activate(pool: PgPool) {
    let f = fixture(pool).await;
    let names: Vec<String> = f
        .mcp
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect();
    for forbidden in ["publish_version", "activate_version", "create_token"] {
        assert!(!names.iter().any(|name| name == forbidden));
    }
    let unknown = f
        .mcp
        .call_tool(CallToolRequestParams::new("publish_version".to_owned()))
        .await;
    assert!(unknown.is_err());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn scopes_are_the_apis_for_every_session(pool: PgPool) {
    let f = fixture(pool).await;
    let http = &f.http;
    let post = |token: Option<String>, body: Value| {
        let mut request = http
            .post(f.server.url("/mcp"))
            .header("accept", "application/json, text/event-stream")
            .json(&body);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        async move { request.send().await.unwrap() }
    };
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });

    let anonymous = post(None, ping.clone()).await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        anonymous.json::<Value>().await.unwrap()["code"],
        "missing_token"
    );

    let unknown = post(Some(format!("bia_{}", "0".repeat(64))), ping.clone()).await;
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);

    let webhook = f.server.webhook_token("greeter", "hook").await;
    let forbidden = post(Some(webhook), ping).await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        forbidden.json::<Value>().await.unwrap()["code"],
        "insufficient_scope"
    );

    // A Token revoked after the session began stops working at once.
    let created = f.server.created_token(Scope::Agent).await;
    let mcp = connect(&f.server, &created.value).await;
    assert_eq!(
        ok(&mcp, "list_workflows", json!({})).await,
        json!({ "workflows": [] })
    );
    f.server
        .client
        .revoke_token(created.token.id)
        .await
        .unwrap();
    let result = mcp
        .call_tool(CallToolRequestParams::new("list_workflows".to_owned()))
        .await;
    assert!(result.is_err() || result.unwrap().is_error == Some(true));
}
