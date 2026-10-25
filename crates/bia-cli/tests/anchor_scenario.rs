//! The M2 anchor scenario, end to end through public interfaces only: an agent authors an
//! uptime-check Workflow over MCP, fixes a Diagnostic, dry-runs it and is refused when it tries
//! to publish; an admin publishes and activates with the CLI; cron fires on a test Clock, the
//! target fails, and the agent finds the failed Run, reads its Timeline and retries it once the
//! target has recovered (ADR-0007, ADR-0008, ADR-0022).
//!
//! Injecting the Clock into the in-process server, scheduler and Worker is configuration; the
//! scenario itself only talks MCP, the HTTP API (`bia-client`) and the `bia` binary.

#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bia_client::Client;
use bia_core::action::CancellationToken;
use bia_core::clock::{Clock, ManualClock};
use bia_engine::{InternalSecrets, Interpreter, Scheduler, SchedulerConfig, Worker, WorkerConfig};
use bia_server::{ApiConfig, ApiState, BootstrapKey};
use bia_store::secret::MasterKey;
use chrono::{DateTime, TimeDelta, Utc};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::process::Command;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const BOOTSTRAP_KEY: &str = "anchor-bootstrap-key-0123456789abcdef";
const SECRET: &str = "uptime-token-7f3a9c1e5b2d4086";
const POLL: TimeDelta = TimeDelta::seconds(1);

type Mcp = RunningService<rmcp::RoleClient, ()>;

async fn connect(url: &str, token: &str) -> Mcp {
    let config = StreamableHttpClientTransportConfig::with_uri(format!("{url}/mcp"))
        .auth_header(token.to_owned());
    ().serve(StreamableHttpClientTransport::from_config(config))
        .await
        .unwrap()
}

async fn call(mcp: &Mcp, tool: &str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object")
    };
    mcp.call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap()
}

/// The structured answer of a tool that succeeded.
async fn ok(mcp: &Mcp, tool: &str, arguments: Value) -> Value {
    let result = call(mcp, tool, arguments).await;
    assert_ne!(result.is_error, Some(true), "{tool} failed: {result:?}");
    result.structured_content.unwrap()
}

/// The uptime target: it requires the Secret, and answers 503 until `recovered` is set.
struct Target {
    recovered: Arc<AtomicBool>,
}

impl Respond for Target {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let authorized = request
            .headers
            .get("authorization")
            .is_some_and(|value| value.to_str().unwrap() == format!("Bearer {SECRET}"));
        if !authorized {
            ResponseTemplate::new(401)
        } else if self.recovered.load(Ordering::SeqCst) {
            ResponseTemplate::new(200).set_body_json(json!({ "status": "up" }))
        } else {
            ResponseTemplate::new(503).set_body_json(json!({ "status": "down" }))
        }
    }
}

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn bia(url: &str, token: &str, args: &[&str], stdin: &str) -> Ran {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bia"))
        .args(args)
        .env_remove("BIA_CONFIG")
        .env("BIA_URL", url)
        .env("BIA_TOKEN", token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin.as_bytes()).await.unwrap();
    drop(input);
    let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .expect("bia did not exit")
        .unwrap();
    Ran {
        code: output.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Polls `check`, which waits on other tasks and never on time: the test moves the Clock.
async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let waited = tokio::time::timeout(Duration::from_secs(30), async {
        while !check().await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "timed out waiting for {what}");
}

fn uptime_check(action: &str, target: &str) -> Value {
    json!({
        "schema_version": 1,
        "id": "uptime-check",
        "triggers": [{ "id": "every_5m", "type": "cron", "expr": "*/5 * * * *" }],
        "steps": [{
            "id": "probe",
            "action": action,
            "params": {
                "method": "GET",
                "url": format!("{target}/health"),
                "headers": { "Authorization": "Bearer {{ secrets.API_TOKEN }}" },
            },
            "timeout": "10s",
        }],
    })
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_agent_authors_an_admin_ships_cron_runs_it_and_the_agent_retries_the_failure(
    pool: PgPool,
) {
    // Configuration: the server, Worker and scheduler share one manual Clock.
    let start: DateTime<Utc> = "2026-11-02T12:00:00Z".parse().unwrap();
    let clock = ManualClock::new(start);
    let registry = Arc::new(bia_actions::builtin_registry().unwrap());
    let key = MasterKey::parse(&"0f".repeat(32)).unwrap();
    let config = ApiConfig {
        bootstrap_key: Some(BootstrapKey::new(BOOTSTRAP_KEY).unwrap()),
        master_key: Some(key.clone()),
        ..ApiConfig::default()
    };
    let state =
        ApiState::new(pool.clone(), registry.clone(), Arc::new(clock.clone())).with_config(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let shutdown = CancellationToken::new();
    let _stop = shutdown.clone().drop_guard();
    tokio::spawn(bia_server::serve(
        listener,
        bia_server::router(state),
        shutdown.clone(),
    ));
    let interpreter = Interpreter::new(pool.clone(), registry, Arc::new(clock.clone()))
        .with_secrets(Arc::new(InternalSecrets::new(pool.clone(), Some(key))));
    let worker = Worker::new("anchor", interpreter, WorkerConfig::default());
    let stop = shutdown.clone();
    tokio::spawn(async move { worker.run(stop).await.unwrap() });

    // The uptime target, down at first.
    let recovered = Arc::new(AtomicBool::new(false));
    let target = MockServer::start().await;
    Mock::given(path("/health"))
        .respond_with(Target {
            recovered: recovered.clone(),
        })
        .mount(&target)
        .await;

    // Setup, as the quickstart does: the admin Token, the agent Token, the Secret.
    let admin = Client::new(&url)
        .unwrap()
        .bootstrap_token(BOOTSTRAP_KEY, "admin")
        .await
        .unwrap()
        .value;
    let created = bia(
        &url,
        &admin,
        &["token", "create", "--name", "claude", "--scope", "agent"],
        "",
    )
    .await;
    assert_eq!(created.code, 0, "{}", created.stderr);
    let agent = created
        .stdout
        .split_whitespace()
        .find(|word| word.starts_with("bia_"))
        .expect("the Token value is printed")
        .to_owned();
    let set = bia(
        &url,
        &admin,
        &["secret", "set", "API_TOKEN"],
        &format!("{SECRET}\n"),
    )
    .await;
    assert_eq!(set.code, 0, "{}", set.stderr);

    let mcp = connect(&url, &agent).await;

    // 1. The agent reads the schema and the Actions, then saves a Draft with a typo.
    let schema = ok(&mcp, "get_schema", json!({})).await;
    assert!(schema["actions"].is_array(), "{schema}");
    let actions = ok(&mcp, "list_actions", json!({})).await;
    let names: Vec<&str> = actions["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| action["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"http.request"), "{names:?}");

    let base = target.uri();
    let saved = ok(
        &mcp,
        "create_draft",
        json!({ "workflow_id": "uptime-check", "definition": uptime_check("http.requst", &base) }),
    )
    .await;
    assert_eq!(saved["version"]["number"], 1);
    assert_eq!(saved["version"]["state"], "draft");

    // 2. Validation returns a Diagnostic with a path and a suggestion; the agent fixes the Draft.
    let report = ok(
        &mcp,
        "validate_workflow",
        json!({ "workflow_id": "uptime-check", "number": 1, "target": "publish" }),
    )
    .await;
    assert_eq!(report["valid"], false, "{report}");
    let diagnostic = &report["diagnostics"][0];
    assert_eq!(diagnostic["path"], "/steps/0/action", "{report}");
    assert!(
        diagnostic["suggestion"]
            .as_str()
            .unwrap()
            .contains("http.request"),
        "{report}"
    );
    ok(
        &mcp,
        "replace_draft",
        json!({
            "workflow_id": "uptime-check",
            "number": 1,
            "definition": uptime_check("http.request", &base),
        }),
    )
    .await;
    let report = ok(
        &mcp,
        "validate_workflow",
        json!({ "workflow_id": "uptime-check", "number": 1, "target": "publish" }),
    )
    .await;
    assert_eq!(report["valid"], true, "{report}");

    // 3. The agent dry-runs the Draft with a Mock and reads the `dry_run` Timeline. Nothing
    // reaches the target and nothing is stored.
    let dry = ok(
        &mcp,
        "dry_run",
        json!({
            "workflow_id": "uptime-check",
            "number": 1,
            "mocks": { "probe": { "output": { "status": 200, "body": { "status": "up" } } } },
        }),
    )
    .await;
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["status"], "succeeded", "{dry}");
    assert_eq!(dry["timeline"][0]["step_id"], "probe");
    assert_eq!(dry["timeline"][0]["output"]["status"], 200);
    assert!(target.received_requests().await.unwrap().is_empty());
    let stored = ok(&mcp, "list_runs", json!({})).await;
    assert!(stored["runs"].as_array().unwrap().is_empty());

    // 4. The agent's publish is refused. MCP has no publish tool (ADR-0048), so the refusal is
    // shown on the CLI, which calls the same API route with the same agent Token.
    let tools: Vec<String> = mcp
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect();
    assert!(
        !tools
            .iter()
            .any(|name| name.contains("publish") || name.contains("activate")),
        "{tools:?}"
    );
    let refused = bia(
        &url,
        &agent,
        &["workflow", "publish", "uptime-check", "1"],
        "",
    )
    .await;
    assert_eq!(refused.code, 1);
    assert!(
        refused.stderr.contains("insufficient_scope"),
        "{}",
        refused.stderr
    );
    let refused = bia(
        &url,
        &agent,
        &["workflow", "activate", "uptime-check", "1"],
        "",
    )
    .await;
    assert_eq!(refused.code, 1);
    assert!(refused.stderr.contains("insufficient_scope"));
    let version = ok(
        &mcp,
        "get_version",
        json!({ "workflow_id": "uptime-check", "number": 1 }),
    )
    .await;
    assert_eq!(version["state"], "draft", "{version}");

    // The human publishes and activates with the admin Token.
    let published = bia(
        &url,
        &admin,
        &["workflow", "publish", "uptime-check", "1"],
        "",
    )
    .await;
    assert_eq!(published.code, 0, "{}", published.stderr);
    let activated = bia(
        &url,
        &admin,
        &["workflow", "activate", "uptime-check", "1"],
        "",
    )
    .await;
    assert_eq!(activated.code, 0, "{}", activated.stderr);
    let workflow = ok(
        &mcp,
        "get_workflow",
        json!({ "workflow_id": "uptime-check" }),
    )
    .await;
    assert_eq!(workflow["active_version"], 1, "{workflow}");

    // 5. Cron fires when the Clock reaches 12:05, the target is down, and the Run fails.
    let scheduler = Scheduler::new(
        "anchor",
        pool.clone(),
        Arc::new(clock.clone()),
        SchedulerConfig {
            poll_interval: POLL,
        },
    );
    let stop = shutdown.clone();
    tokio::spawn(async move { scheduler.run(stop).await.unwrap() });
    let mut waited = TimeDelta::zero();
    while ok(&mcp, "list_runs", json!({})).await["runs"]
        .as_array()
        .unwrap()
        .is_empty()
    {
        assert!(waited < TimeDelta::minutes(6), "cron never fired");
        clock.advance(POLL);
        waited += POLL;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(clock.now() >= start + TimeDelta::minutes(5));
    eventually("the cron Run to fail", || async {
        ok(&mcp, "list_failures", json!({})).await["runs"]
            .as_array()
            .is_some_and(|runs| runs.len() == 1)
    })
    .await;

    // 6. The agent lists failures, reads the Timeline, and retries after the target recovers.
    let failures = ok(&mcp, "list_failures", json!({})).await;
    let failed = &failures["runs"][0];
    assert_eq!(failed["workflow_id"], "uptime-check", "{failed}");
    assert_eq!(failed["status"], "failed");
    let failed_id = failed["id"].as_str().unwrap().to_owned();
    let run = ok(&mcp, "get_run", json!({ "run_id": failed_id })).await;
    let entry = &run["timeline"][0];
    assert_eq!(entry["step_id"], "probe", "{run}");
    assert!(entry["error"].to_string().contains("503"), "{entry}");
    assert_eq!(
        entry["input"]["headers"]["Authorization"],
        "Bearer {{ secrets.API_TOKEN }}"
    );
    assert!(!run.to_string().contains(SECRET));

    recovered.store(true, Ordering::SeqCst);
    let retried = ok(&mcp, "retry_run", json!({ "run_id": failed_id })).await;
    let retried_id = retried["id"].as_str().unwrap().to_owned();
    assert_ne!(retried_id, failed_id);
    eventually("the retried Run to succeed", || async {
        ok(&mcp, "get_run", json!({ "run_id": retried_id })).await["status"] == "succeeded"
    })
    .await;
    let run = ok(&mcp, "get_run", json!({ "run_id": retried_id })).await;
    assert_eq!(run["retry_of"], json!(failed_id), "{run}");
    assert_eq!(run["timeline"][0]["output"]["body"]["status"], "up");
    assert!(!run.to_string().contains(SECRET));
    let original = ok(&mcp, "get_run", json!({ "run_id": failed_id })).await;
    assert_eq!(original["status"], "failed");
}
