//! The API commands of the `bia` binary, driven against an in-process server with a Worker:
//! applying Definition files, validating, Dry Runs, publishing, activating and Runs.

#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use bia_client::{Client, NewToken, Scope, VersionState};
use bia_core::action::CancellationToken;
use bia_engine::{Interpreter, Worker, WorkerConfig};
use bia_server::{ApiConfig, ApiState, BootstrapKey};
use bia_store::SystemClock;
use serde_json::{Value, json};
use sqlx::PgPool;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::Command;

const BOOTSTRAP_KEY: &str = "cli-test-bootstrap-key-0123456789abcdef";

struct Harness {
    url: String,
    admin: String,
    agent: String,
    client: Client,
    files: TempDir,
    shutdown: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn start(pool: PgPool) -> Harness {
    let registry = Arc::new(bia_actions::builtin_registry().unwrap());
    let config = ApiConfig {
        bootstrap_key: Some(BootstrapKey::new(BOOTSTRAP_KEY).unwrap()),
        ..ApiConfig::default()
    };
    let state =
        ApiState::new(pool.clone(), registry.clone(), Arc::new(SystemClock)).with_config(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let shutdown = CancellationToken::new();
    tokio::spawn(bia_server::serve(
        listener,
        bia_server::router(state),
        shutdown.clone(),
    ));
    let interpreter = Interpreter::new(pool, registry, Arc::new(SystemClock));
    let worker = Worker::new("cli-test", interpreter, WorkerConfig::default());
    let stop = shutdown.clone();
    tokio::spawn(async move { worker.run(stop).await.unwrap() });

    let anonymous = Client::new(&url).unwrap();
    let admin = anonymous
        .bootstrap_token(BOOTSTRAP_KEY, "admin")
        .await
        .unwrap()
        .value;
    let client = anonymous.with_token(&admin);
    let agent = client
        .create_token(&NewToken {
            name: "agent".into(),
            scope: Scope::Agent,
            webhook: None,
        })
        .await
        .unwrap()
        .value;
    Harness {
        url,
        admin,
        agent,
        client,
        files: TempDir::new().unwrap(),
        shutdown,
    }
}

struct Ran {
    output: Output,
}

impl Ran {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout())
            .unwrap_or_else(|_| panic!("not JSON: {}\n{}", self.stdout(), self.stderr()))
    }

    fn code(&self) -> i32 {
        self.output.status.code().unwrap()
    }
}

impl Harness {
    fn write(&self, path: &str, text: &str) -> std::path::PathBuf {
        let path = self.files.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    async fn bia_with(&self, token: &str, args: &[&str]) -> Ran {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bia"));
        command
            .args(args)
            .env_remove("BIA_CONFIG")
            .env("BIA_URL", &self.url)
            .env("BIA_TOKEN", token)
            .current_dir(self.files.path())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(60), command.output())
            .await
            .expect("bia did not exit")
            .unwrap();
        Ran { output }
    }

    /// Runs `bia` with the admin Token.
    async fn bia(&self, args: &[&str]) -> Ran {
        self.bia_with(&self.admin, args).await
    }
}

const PRICE_YAML: &str = "\
schema_version: 1
id: price
triggers:
  - id: by_hand
    type: manual
steps:
  - id: total
    action: transform.expr
    params:
      expression: values.price * 2.0
      values:
        price: 21.0
  - id: done
    action: flow.succeed
";

fn greet_json() -> String {
    json!({
        "schema_version": 1,
        "id": "greet",
        "steps": [
            { "id": "hello", "action": "transform.expr", "params": { "expression": "1 + 1" } },
            { "id": "done", "action": "flow.succeed" },
        ],
    })
    .to_string()
}

const BAD_YAML: &str = "\
schema_version: 1
id: bad
steps:
  - id: first
    action: flow.succeed
  - id: second
    action: nope.nothing
";

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn apply_creates_drafts_from_mixed_json_and_yaml_in_nested_directories(pool: PgPool) {
    let h = start(pool).await;
    h.write("flows/price.yaml", PRICE_YAML);
    h.write("flows/deep/greet.json", &greet_json());
    h.write("flows/notes.txt", "not a Definition");

    let applied = h.bia(&["workflow", "apply", "-f", "flows", "--json"]).await;
    assert_eq!(applied.code(), 0, "{}", applied.stderr());
    let report = applied.json();
    let results = report["applied"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r["result"] == "created"));

    for id in ["price", "greet"] {
        let versions = h.client.list_versions(id).await.unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].state, VersionState::Draft);
    }
    let stored = h.client.get_version("price", 1).await.unwrap();
    assert_eq!(stored.definition["steps"][0]["id"], "total");

    let again = h.bia(&["workflow", "apply", "-f", "flows"]).await;
    assert_eq!(again.code(), 0, "{}", again.stderr());
    assert!(
        again.stdout().contains("replaced price v1"),
        "{}",
        again.stdout()
    );
    assert_eq!(h.client.list_versions("price").await.unwrap().len(), 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn apply_with_activate_publishes_and_activates_then_is_idempotent(pool: PgPool) {
    let h = start(pool).await;
    let file = h.write("price.yaml", PRICE_YAML);
    let file = file.to_str().unwrap();

    let first = h
        .bia(&["workflow", "apply", "-f", file, "--activate"])
        .await;
    assert_eq!(first.code(), 0, "{}", first.stderr());
    assert!(
        first.stdout().contains("published, activated"),
        "{}",
        first.stdout()
    );
    let workflow = h.client.get_workflow("price").await.unwrap();
    assert_eq!(workflow.active_version, Some(1));

    let second = h
        .bia(&["workflow", "apply", "-f", file, "--activate", "--json"])
        .await;
    assert_eq!(second.code(), 0, "{}", second.stderr());
    assert_eq!(second.json()["applied"][0]["result"], "unchanged");
    assert_eq!(h.client.list_versions("price").await.unwrap().len(), 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn apply_without_a_scope_for_publishing_fails_clearly_for_an_agent_token(pool: PgPool) {
    let h = start(pool).await;
    let file = h.write("price.yaml", PRICE_YAML);
    let file = file.to_str().unwrap();

    let ran = h
        .bia_with(&h.agent, &["workflow", "apply", "-f", file, "--publish"])
        .await;
    assert_eq!(ran.code(), 1);
    assert!(
        ran.stdout().contains("insufficient_scope"),
        "{}",
        ran.stdout()
    );
    assert!(ran.stdout().contains("admin"), "{}", ran.stdout());
    // The Draft was saved; only publishing was refused.
    assert_eq!(h.client.list_versions("price").await.unwrap().len(), 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn validate_prints_each_diagnostic_with_its_yaml_line_and_exits_non_zero(pool: PgPool) {
    let h = start(pool).await;
    h.write("bad.yaml", BAD_YAML);

    let ran = h.bia(&["workflow", "validate", "-f", "bad.yaml"]).await;
    assert_eq!(ran.code(), 1);
    let out = ran.stdout();
    assert!(
        out.contains("bad.yaml:7: error[unknown_action] /steps/1/action"),
        "{out}"
    );
    assert!(out.contains("bad.yaml: invalid, 1 error"), "{out}");

    let json = h
        .bia(&["workflow", "validate", "-f", "bad.yaml", "--json"])
        .await;
    assert_eq!(json.code(), 1);
    let report = json.json();
    let result = &report["results"][0];
    assert_eq!(result["valid"], false);
    assert_eq!(result["diagnostics"][0]["code"], "unknown_action");
    assert_eq!(result["diagnostics"][0]["path"], "/steps/1/action");

    h.write("good.yaml", PRICE_YAML);
    let good = h.bia(&["workflow", "validate", "-f", "good.yaml"]).await;
    assert_eq!(good.code(), 0, "{}", good.stdout());
    assert!(good.stdout().contains("good.yaml: valid"));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn publish_and_activate_need_an_admin_token(pool: PgPool) {
    let h = start(pool).await;
    let file = h.write("price.yaml", PRICE_YAML);
    h.bia(&["workflow", "apply", "-f", file.to_str().unwrap()])
        .await;

    let refused = h
        .bia_with(&h.agent, &["workflow", "publish", "price", "1"])
        .await;
    assert_eq!(refused.code(), 1);
    assert!(
        refused.stderr().contains("insufficient_scope"),
        "{}",
        refused.stderr()
    );
    assert!(refused.stderr().contains("admin"), "{}", refused.stderr());

    let published = h.bia(&["workflow", "publish", "price", "1"]).await;
    assert_eq!(published.code(), 0, "{}", published.stderr());
    let refused = h
        .bia_with(&h.agent, &["workflow", "activate", "price", "1"])
        .await;
    assert_eq!(refused.code(), 1);
    let activated = h
        .bia(&["workflow", "activate", "price", "1", "--json"])
        .await;
    assert_eq!(activated.code(), 0, "{}", activated.stderr());
    assert_eq!(activated.json()["active_version"], 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn publishing_an_invalid_draft_shows_the_diagnostics(pool: PgPool) {
    let h = start(pool).await;
    h.bia(&[
        "workflow",
        "apply",
        "-f",
        h.write("bad.yaml", BAD_YAML).to_str().unwrap(),
    ])
    .await;
    let ran = h.bia(&["workflow", "publish", "bad", "1"]).await;
    assert_eq!(ran.code(), 1);
    assert!(
        ran.stderr().contains("error[invalid_definition]"),
        "{}",
        ran.stderr()
    );
    assert!(ran.stderr().contains("unknown_action"), "{}", ran.stderr());
    let json = h.bia(&["workflow", "publish", "bad", "1", "--json"]).await;
    assert_eq!(
        json.json()["error"]["diagnostics"][0]["code"],
        "unknown_action"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn runs_can_be_triggered_shown_listed_and_retried(pool: PgPool) {
    let h = start(pool).await;
    let file = h.write("price.yaml", PRICE_YAML);
    let applied = h
        .bia(&[
            "workflow",
            "apply",
            "-f",
            file.to_str().unwrap(),
            "--activate",
        ])
        .await;
    assert_eq!(applied.code(), 0, "{}", applied.stderr());

    let started = h.bia(&["run", "trigger", "price", "--json"]).await;
    assert_eq!(started.code(), 0, "{}", started.stderr());
    let id = started.json()["id"].as_str().unwrap().to_owned();

    let mut shown = h.bia(&["run", "get", &id, "--json"]).await;
    for _ in 0..500 {
        if shown.json()["status"] == "succeeded" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        shown = h.bia(&["run", "get", &id, "--json"]).await;
    }
    let run = shown.json();
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["timeline"][0]["step_id"], "total");
    assert_eq!(run["timeline"][0]["output"], json!(42.0));

    let text = h.bia(&["run", "get", &id]).await;
    assert!(text.stdout().contains("total"), "{}", text.stdout());
    assert!(text.stdout().contains("succeeded"), "{}", text.stdout());

    let listed = h
        .bia(&[
            "run",
            "list",
            "--workflow",
            "price",
            "--status",
            "succeeded",
            "--json",
        ])
        .await;
    assert_eq!(listed.json()["runs"][0]["id"], id);
    let table = h.bia(&["run", "list"]).await;
    assert!(table.stdout().contains(&id), "{}", table.stdout());

    let retried = h.bia(&["run", "retry", &id, "--json"]).await;
    assert_eq!(retried.code(), 0, "{}", retried.stderr());
    assert_eq!(retried.json()["retry_of"], id);

    // Cancelling a finished Run is refused by the API, so the CLI fails.
    let cancel = h.bia(&["run", "cancel", &id]).await;
    assert_eq!(cancel.code(), 1);
    assert!(
        cancel.stderr().contains("run_finished"),
        "{}",
        cancel.stderr()
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn triggering_a_workflow_without_an_active_version_fails_with_the_api_code(pool: PgPool) {
    let h = start(pool).await;
    h.bia(&[
        "workflow",
        "apply",
        "-f",
        h.write("price.yaml", PRICE_YAML).to_str().unwrap(),
    ])
    .await;
    let ran = h.bia(&["run", "trigger", "price"]).await;
    assert_eq!(ran.code(), 1);
    assert!(
        ran.stderr().contains("no_active_version"),
        "{}",
        ran.stderr()
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn dry_run_returns_the_timeline_with_mocks_from_files(pool: PgPool) {
    let h = start(pool).await;
    h.write(
        "fetch.yaml",
        "schema_version: 1\nid: fetch\nsteps:\n  - id: get\n    action: http.request\n    params:\n      url: https://example.com/health\n  - id: done\n    action: flow.succeed\n",
    );
    h.write(
        "mocks.json",
        r#"{ "get": { "output": { "status": 200, "body": "ok" } } }"#,
    );

    let ran = h
        .bia(&[
            "workflow",
            "dry-run",
            "-f",
            "fetch.yaml",
            "--mocks",
            "mocks.json",
            "--json",
        ])
        .await;
    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    let report = ran.json();
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["status"], "succeeded");
    assert_eq!(report["timeline"][0]["output"]["status"], 200);

    let text = h
        .bia(&[
            "workflow",
            "dry-run",
            "-f",
            "fetch.yaml",
            "--mocks",
            "mocks.json",
        ])
        .await;
    assert!(
        text.stdout().contains("dry run succeeded"),
        "{}",
        text.stdout()
    );
    assert!(text.stdout().contains("get"), "{}", text.stdout());

    // An unknown Step in the Mocks is refused with its code.
    h.write("typo.json", r#"{ "nope": { "output": 1 } }"#);
    let typo = h
        .bia(&[
            "workflow",
            "dry-run",
            "-f",
            "fetch.yaml",
            "--mocks",
            "typo.json",
        ])
        .await;
    assert_eq!(typo.code(), 1);
    assert!(typo.stderr().contains("invalid_mock"), "{}", typo.stderr());

    // A stored Version is dry-run by Workflow and number.
    h.bia(&["workflow", "apply", "-f", "fetch.yaml"]).await;
    let stored = h
        .bia(&[
            "workflow",
            "dry-run",
            "fetch",
            "1",
            "--mocks",
            "mocks.json",
            "--json",
        ])
        .await;
    assert_eq!(stored.code(), 0, "{}", stored.stderr());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_failed_dry_run_exits_non_zero(pool: PgPool) {
    let h = start(pool).await;
    h.write(
        "fetch.yaml",
        "schema_version: 1\nid: fetch\nsteps:\n  - id: get\n    action: http.request\n    params:\n      url: https://example.com/health\n  - id: done\n    action: flow.succeed\n",
    );
    h.write(
        "fail.json",
        r#"{ "get": { "error": { "class": "fatal", "message": "boom" } } }"#,
    );
    let ran = h
        .bia(&[
            "workflow",
            "dry-run",
            "-f",
            "fetch.yaml",
            "--mocks",
            "fail.json",
        ])
        .await;
    assert_eq!(ran.code(), 1);
    assert!(ran.stdout().contains("dry run failed"), "{}", ran.stdout());
    assert!(ran.stdout().contains("boom"), "{}", ran.stdout());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn diff_shows_the_changes_between_versions(pool: PgPool) {
    let h = start(pool).await;
    let file = h.write("price.yaml", PRICE_YAML);
    let file = file.to_str().unwrap();
    h.bia(&["workflow", "apply", "-f", file, "--publish"]).await;
    h.write("price.yaml", &PRICE_YAML.replace("21.0", "50.0"));
    let applied = h.bia(&["workflow", "apply", "-f", file]).await;
    assert!(
        applied.stdout().contains("created price v2"),
        "{}",
        applied.stdout()
    );

    let diff = h.bia(&["workflow", "diff", "price", "1", "2"]).await;
    assert_eq!(diff.code(), 0, "{}", diff.stderr());
    assert!(
        diff.stdout()
            .contains("~ /steps/0/params/values/price 50.0"),
        "{}",
        diff.stdout()
    );
    let json = h
        .bia(&["workflow", "diff", "price", "1", "2", "--json"])
        .await;
    assert_eq!(json.json()["patch"][0]["op"], "replace");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn url_and_token_come_from_the_config_file_and_the_flag_wins(pool: PgPool) {
    let h = start(pool).await;
    let config = h.write(
        "bia.toml",
        &format!("url = \"{}\"\ntoken = \"{}\"\n", h.url, h.admin),
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_bia"));
    command
        .args([
            "--config",
            config.to_str().unwrap(),
            "workflow",
            "list",
            "--json",
        ])
        .env_remove("BIA_URL")
        .env_remove("BIA_TOKEN")
        .kill_on_drop(true);
    let ran = Ran {
        output: command.output().await.unwrap(),
    };
    assert_eq!(ran.code(), 0, "{}", ran.stderr());
    assert_eq!(ran.json(), json!({ "workflows": [] }));

    let unreachable = h
        .bia(&["--url", "http://127.0.0.1:1", "workflow", "list"])
        .await;
    assert_eq!(unreachable.code(), 1);
    assert!(
        unreachable.stderr().contains("cannot reach the server"),
        "{}",
        unreachable.stderr()
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_missing_token_says_how_to_set_one(pool: PgPool) {
    let h = start(pool).await;
    let ran = h.bia_with("", &["workflow", "list"]).await;
    assert_eq!(ran.code(), 1);
    assert!(ran.stderr().contains("BIA_TOKEN"), "{}", ran.stderr());
}

#[test]
fn files_that_do_not_parse_stop_apply_before_anything_is_sent() {
    // No server is reachable here: the failure must come from reading the file.
    let dir = TempDir::new().unwrap();
    write_all(dir.path(), "broken.yaml", "id: [unclosed\n");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_bia"))
        .args(["workflow", "apply", "-f", "broken.yaml"])
        .env("BIA_URL", "http://127.0.0.1:1")
        .env_remove("BIA_CONFIG")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("broken.yaml: invalid YAML"), "{stderr}");
}

/// A running `bia mcp` whose stdio the test speaks JSON-RPC over.
struct McpProxy {
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    _child: tokio::process::Child,
}

impl McpProxy {
    fn start(h: &Harness, token: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bia"))
            .arg("mcp")
            .env_remove("BIA_CONFIG")
            .env("BIA_URL", &h.url)
            .env("BIA_TOKEN", token)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Self {
            stdin: child.stdin.take().unwrap(),
            lines: BufReader::new(child.stdout.take().unwrap()).lines(),
            _child: child,
        }
    }

    /// Sends a request and returns its reply.
    async fn request(&mut self, id: u32, method: &str, params: Value) -> Value {
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send(&message).await;
        let line = tokio::time::timeout(Duration::from_secs(30), self.lines.next_line())
            .await
            .expect("no reply from bia mcp")
            .unwrap()
            .expect("bia mcp closed stdout");
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id, "{reply}");
        reply
    }

    async fn send(&mut self, message: &Value) {
        self.stdin
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn initialize(&mut self) -> Value {
        let reply = self
            .request(
                1,
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "0" },
                }),
            )
            .await;
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;
        reply
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn bia_mcp_proxies_a_full_session_to_the_server(pool: PgPool) {
    let h = start(pool).await;
    let mut mcp = McpProxy::start(&h, &h.agent);

    let initialized = mcp.initialize().await;
    assert!(initialized["result"]["capabilities"]["tools"].is_object());

    let listed = mcp.request(2, "tools/list", json!({})).await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"validate_workflow"));
    assert!(!names.contains(&"publish_version"));

    let definition: Value = serde_json::from_str(&greet_json()).unwrap();
    let saved = mcp
        .request(
            3,
            "tools/call",
            json!({
                "name": "create_draft",
                "arguments": { "workflow_id": "greet", "definition": definition },
            }),
        )
        .await;
    assert_eq!(saved["result"]["isError"], false, "{saved}");
    assert_eq!(saved["result"]["structuredContent"]["version"]["number"], 1);
    assert_eq!(h.client.list_versions("greet").await.unwrap().len(), 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn bia_mcp_reports_a_refused_token_as_a_json_rpc_error(pool: PgPool) {
    let h = start(pool).await;
    let mut mcp = McpProxy::start(&h, "not-a-token");
    let reply = mcp.request(1, "tools/list", json!({})).await;
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(message.contains("invalid_token"), "{reply}");
}

fn write_all(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

const EMAILS_QUEUE_YAML: &str = "\
kind: queue
name: emails
max_retries: 5
visibility_timeout: 2m
dead_letter: failed-emails
";

const SENDER_YAML: &str = "\
schema_version: 1
id: sender
triggers:
  - id: by_hand
    type: manual
steps:
  - id: send
    action: queue.publish
    params:
      queue: emails
      body: hello
";

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn apply_creates_queues_before_the_definitions_that_use_them_and_updates_them(pool: PgPool) {
    let h = start(pool).await;
    h.write("setup/emails.yaml", EMAILS_QUEUE_YAML);
    h.write(
        "setup/failed-emails.yaml",
        "kind: queue\nname: failed-emails\n",
    );
    h.write("setup/sender.yaml", SENDER_YAML);

    let applied = h
        .bia(&["workflow", "apply", "-f", "setup", "--activate", "--json"])
        .await;
    assert_eq!(applied.code(), 0, "{}", applied.stdout());
    let report = applied.json();
    let queues: Vec<_> = report["queues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| (q["queue"].as_str().unwrap(), q["result"].as_str().unwrap()))
        .collect();
    assert_eq!(
        queues,
        [("failed-emails", "created"), ("emails", "created")]
    );
    assert_eq!(report["applied"][0]["activated"], true);
    let emails = h.client.get_queue("emails").await.unwrap();
    assert_eq!(
        (
            emails.max_retries,
            emails.visibility_timeout.as_str(),
            emails.batch_size
        ),
        (5, "2m", 10)
    );
    assert_eq!(emails.dead_letter.as_deref(), Some("failed-emails"));

    h.write(
        "setup/emails.yaml",
        "kind: queue\nname: emails\nmax_retries: 1\n",
    );
    let again = h.bia(&["workflow", "apply", "-f", "setup"]).await;
    assert_eq!(again.code(), 0, "{}", again.stderr());
    assert!(
        again.stdout().contains("updated queue emails"),
        "{}",
        again.stdout()
    );
    assert!(
        again.stdout().contains("unchanged queue failed-emails"),
        "{}",
        again.stdout()
    );
    let emails = h.client.get_queue("emails").await.unwrap();
    assert_eq!((emails.max_retries, emails.dead_letter), (1, None));

    let validated = h
        .bia(&["workflow", "validate", "-f", "setup", "--json"])
        .await;
    assert_eq!(validated.code(), 0, "{}", validated.stdout());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_bad_queue_file_is_refused_before_anything_is_applied(pool: PgPool) {
    let h = start(pool).await;
    h.write(
        "bad/emails.yaml",
        "kind: queue\nname: emails\nmax_retires: 5\n",
    );
    h.write("bad/sender.yaml", SENDER_YAML);

    let applied = h.bia(&["workflow", "apply", "-f", "bad"]).await;
    assert_ne!(applied.code(), 0);
    assert!(
        applied.stderr().contains("max_retires"),
        "{}",
        applied.stderr()
    );
    assert!(h.client.list_queues().await.unwrap().is_empty());
    assert!(h.client.list_workflows().await.unwrap().is_empty());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn queue_commands_set_list_show_and_delete_queues(pool: PgPool) {
    let h = start(pool).await;

    let set = h
        .bia(&[
            "queue",
            "set",
            "jobs",
            "--max-retries",
            "0",
            "--visibility-timeout",
            "45s",
        ])
        .await;
    assert_eq!(set.code(), 0, "{}", set.stderr());
    let listed = h.bia_with(&h.agent, &["queue", "list"]).await;
    assert_eq!(listed.code(), 0, "{}", listed.stderr());
    assert!(listed.stdout().contains("jobs"), "{}", listed.stdout());
    assert!(listed.stdout().contains("45s"), "{}", listed.stdout());
    let shown = h
        .bia_with(&h.agent, &["queue", "get", "jobs", "--json"])
        .await;
    assert_eq!(shown.json()["max_retries"], 0);
    let messages = h.bia(&["queue", "messages", "jobs", "--json"]).await;
    assert_eq!(messages.json()["messages"], json!([]));

    let refused = h.bia_with(&h.agent, &["queue", "delete", "jobs"]).await;
    assert_ne!(refused.code(), 0);
    assert!(
        refused.stderr().contains("insufficient_scope"),
        "{}",
        refused.stderr()
    );
    let deleted = h.bia(&["queue", "delete", "jobs"]).await;
    assert_eq!(deleted.code(), 0, "{}", deleted.stderr());
    let missing = h.bia(&["queue", "get", "jobs"]).await;
    assert!(
        missing.stderr().contains("queue_not_found"),
        "{}",
        missing.stderr()
    );
}
