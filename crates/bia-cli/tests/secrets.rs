//! Secrets end to end: `bia secret` sets and lists them, a Worker resolves `secrets.NAME` into
//! a real `http.request`, and the value never reaches the Timeline, the Run or any output
//! (ADR-0017, ADR-0049).

#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use bia_client::{Client, NewToken, Run, RunStatus, Scope};
use bia_core::action::CancellationToken;
use bia_engine::{InternalSecrets, Interpreter, Worker, WorkerConfig};
use bia_server::{ApiConfig, ApiState, BootstrapKey};
use bia_store::SystemClock;
use bia_store::secret::MasterKey;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::process::Command;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const BOOTSTRAP_KEY: &str = "cli-test-bootstrap-key-0123456789abcdef";
const SECRET: &str = "tok-7f3a9c1e5b2d4086";

struct Harness {
    url: String,
    admin: String,
    client: Client,
    shutdown: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn start(pool: PgPool) -> Harness {
    let registry = Arc::new(bia_actions::builtin_registry().unwrap());
    let key = MasterKey::parse(&"0f".repeat(32)).unwrap();
    let config = ApiConfig {
        bootstrap_key: Some(BootstrapKey::new(BOOTSTRAP_KEY).unwrap()),
        master_key: Some(key.clone()),
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
    let interpreter = Interpreter::new(pool.clone(), registry, Arc::new(SystemClock))
        .with_secrets(Arc::new(InternalSecrets::new(pool, Some(key))));
    let worker = Worker::new("secrets-test", interpreter, WorkerConfig::default());
    let stop = shutdown.clone();
    tokio::spawn(async move { worker.run(stop).await.unwrap() });

    let anonymous = Client::new(&url).unwrap();
    let admin = anonymous
        .bootstrap_token(BOOTSTRAP_KEY, "admin")
        .await
        .unwrap()
        .value;
    Harness {
        client: anonymous.with_token(&admin),
        url,
        admin,
        shutdown,
    }
}

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Harness {
    /// Runs `bia` with `token`, writing `stdin` to it.
    async fn bia(&self, token: &str, args: &[&str], stdin: &str) -> Ran {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bia"))
            .args(args)
            .env_remove("BIA_CONFIG")
            .env("BIA_URL", &self.url)
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

    async fn set(&self, name: &str, value: &str) {
        let set = self.bia(&self.admin, &["secret", "set", name], value).await;
        assert_eq!(set.code, 0, "{}", set.stderr);
    }

    /// Activates a Workflow with one `http.request` Step whose `Authorization` header reads
    /// `secrets.API_TOKEN`, runs it once, and returns the finished Run.
    async fn call(&self, id: &str, url: &str) -> Run {
        let definition = json!({
            "schema_version": 1,
            "id": id,
            "triggers": [{ "id": "by_hand", "type": "manual" }],
            "steps": [{
                "id": "check",
                "action": "http.request",
                "params": {
                    "url": url,
                    "headers": { "Authorization": "Bearer {{ secrets.API_TOKEN }}" },
                },
            }],
        });
        let saved = self.client.create_draft(id, &definition).await.unwrap();
        let number = saved.version.summary.number;
        self.client.publish_version(id, number).await.unwrap();
        self.client.activate_version(id, number).await.unwrap();
        let run = self.client.trigger_run(id, &json!({}), None).await.unwrap();
        for _ in 0..500 {
            let shown = self.client.get_run(run.id).await.unwrap();
            if shown.summary.status.is_finished() {
                return shown;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the Run did not finish");
    }
}

/// Answers 200, or 403 for `/denied`, with the `Authorization` header it received as the body.
struct EchoAuthorization;

impl Respond for EchoAuthorization {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let received = request
            .headers
            .get("authorization")
            .map(|value| value.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let status = if request.url.path() == "/denied" {
            403
        } else {
            200
        };
        ResponseTemplate::new(status).set_body_json(json!({ "you_sent": received }))
    }
}

fn timeline_text(run: &Run) -> String {
    serde_json::to_string(&run.timeline).unwrap() + &run.input.to_string()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_run_sends_the_real_secret_while_the_timeline_shows_only_the_template(pool: PgPool) {
    let h = start(pool).await;
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/up"))
        .and(header("authorization", format!("Bearer {SECRET}").as_str()))
        .respond_with(EchoAuthorization)
        .expect(1)
        .mount(&upstream)
        .await;
    h.set("API_TOKEN", &format!("{SECRET}\n")).await;

    let run = h.call("uptime", &format!("{}/up", upstream.uri())).await;

    assert_eq!(run.summary.status, RunStatus::Succeeded, "{run:?}");
    let entry = &run.timeline[0];
    assert_eq!(
        entry.input.as_ref().unwrap()["headers"]["Authorization"],
        "Bearer {{ secrets.API_TOKEN }}"
    );
    assert_eq!(
        entry.output.as_ref().unwrap()["body"]["you_sent"],
        "Bearer ***"
    );
    assert!(!timeline_text(&run).contains(SECRET), "{run:?}");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_error_that_echoes_the_secret_is_masked_in_the_timeline(pool: PgPool) {
    let h = start(pool).await;
    let upstream = MockServer::start().await;
    Mock::given(path("/denied"))
        .respond_with(EchoAuthorization)
        .mount(&upstream)
        .await;
    h.set("API_TOKEN", SECRET).await;

    let run = h
        .call("denied", &format!("{}/denied", upstream.uri()))
        .await;

    assert_eq!(run.summary.status, RunStatus::Failed);
    let error = run.timeline[0].error.as_ref().unwrap();
    assert!(error.to_string().contains("Bearer ***"), "{error}");
    assert!(!timeline_text(&run).contains(SECRET), "{run:?}");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_missing_secret_fails_the_step_naming_it(pool: PgPool) {
    let h = start(pool).await;

    let run = h.call("missing", "http://127.0.0.1:9/never").await;

    assert_eq!(run.summary.status, RunStatus::Failed);
    let error = run.timeline[0].error.as_ref().unwrap();
    assert_eq!(error["code"], "unknown_secret");
    assert_eq!(error["class"], "fatal");
    assert!(error["message"].as_str().unwrap().contains("API_TOKEN"));
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_cli_sets_lists_and_deletes_secrets_without_ever_printing_a_value(pool: PgPool) {
    let h = start(pool).await;

    let set = h
        .bia(
            &h.admin,
            &["secret", "set", "API_TOKEN"],
            &format!("{SECRET}\n"),
        )
        .await;
    assert_eq!(set.code, 0, "{}", set.stderr);
    let list = h.bia(&h.admin, &["secret", "list", "--json"], "").await;
    let table = h.bia(&h.admin, &["secret", "list"], "").await;
    let deleted = h
        .bia(&h.admin, &["secret", "delete", "API_TOKEN"], "")
        .await;
    let gone = h
        .bia(&h.admin, &["secret", "delete", "API_TOKEN"], "")
        .await;
    let empty = h.bia(&h.admin, &["secret", "set", "EMPTY"], "\n").await;

    let listed: Value = serde_json::from_str(&list.stdout).unwrap();
    assert_eq!(listed["secrets"][0]["name"], "API_TOKEN");
    assert!(table.stdout.contains("API_TOKEN"), "{}", table.stdout);
    assert_eq!(deleted.code, 0, "{}", deleted.stderr);
    assert_eq!(gone.code, 1);
    assert!(gone.stderr.contains("secret_not_found"), "{}", gone.stderr);
    assert_eq!(empty.code, 1);
    for ran in [&set, &list, &table, &deleted, &gone, &empty] {
        assert!(!ran.stdout.contains(SECRET) && !ran.stderr.contains(SECRET));
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_agent_token_cannot_set_or_delete_secrets_through_the_cli(pool: PgPool) {
    let h = start(pool).await;
    h.set("API_TOKEN", SECRET).await;
    let agent = h
        .client
        .create_token(&NewToken {
            name: "agent".into(),
            scope: Scope::Agent,
            webhook: None,
        })
        .await
        .unwrap()
        .value;

    let set = h.bia(&agent, &["secret", "set", "OTHER"], "x").await;
    let delete = h.bia(&agent, &["secret", "delete", "API_TOKEN"], "").await;
    let list = h.bia(&agent, &["secret", "list"], "").await;

    for denied in [&set, &delete] {
        assert_eq!(denied.code, 1);
        assert!(
            denied.stderr.contains("insufficient_scope"),
            "{}",
            denied.stderr
        );
    }
    assert_eq!(list.code, 0, "{}", list.stderr);
    assert_eq!(h.client.list_secrets().await.unwrap().len(), 1);
}
