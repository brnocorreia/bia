//! The `bia` binary migrates an empty database, runs a Run to completion as a Worker, leads as a
//! scheduler, serves the API, logs a Setup Link that `bia setup` uses, resets the admin, stops
//! on SIGTERM, and refuses to start with invalid configuration.

#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::process::{Output, Stdio};
use std::time::Duration;

use bia_core::clock::Clock;
use bia_core::dsl::Definition;
use bia_engine::start_run;
use bia_store::SystemClock;
use bia_store::run::{PgRunRepository, RunRepository, RunStatus};
use bia_store::timeline::{PgTimelineRepository, TimelineRepository};
use bia_store::version::{PgVersionRepository, VersionRepository};
use bia_store::workflow::{PgWorkflowRepository, WorkflowRepository};
use serde_json::json;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

fn bia() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bia"));
    command
        .env_remove("BIA_CONFIG")
        .env_remove("DATABASE_URL")
        .kill_on_drop(true);
    command
}

/// The URL of the database `pool` was made for, on the server `DATABASE_URL` names.
fn database_url(pool: &PgPool) -> String {
    let server = std::env::var("DATABASE_URL").unwrap();
    let (server, _) = server.rsplit_once('/').unwrap();
    let database = pool.connect_options().get_database().unwrap().to_owned();
    format!("{server}/{database}")
}

async fn finished(command: &mut Command) -> Output {
    tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .expect("bia did not exit")
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn bia_migrate_then_serve_runs_a_seeded_run_to_completion(pool: PgPool) {
    let url = database_url(&pool);

    let migrated = finished(bia().arg("migrate").env("BIA_DATABASE_URL", &url)).await;
    assert!(migrated.status.success(), "{migrated:?}");
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        applied,
        i64::try_from(bia_store::MIGRATOR.iter().count()).unwrap()
    );

    let definition = Definition::from_value(json!({
        "schema_version": 1,
        "id": "price",
        "steps": [
            {
                "id": "total",
                "action": "transform.expr",
                "params": { "expression": "values.price * 2.0", "values": { "price": 21.0 } },
            },
            { "id": "done", "action": "flow.succeed" },
        ],
    }))
    .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    PgWorkflowRepository
        .create(&mut conn, &definition.id)
        .await
        .unwrap();
    let draft = PgVersionRepository
        .create_draft(&mut conn, &definition)
        .await
        .unwrap();
    let now = SystemClock.now();
    PgVersionRepository
        .publish(&mut conn, &draft.workflow_id, draft.number, now)
        .await
        .unwrap();
    let run = start_run(
        &mut conn,
        &definition.id,
        draft.number,
        json!({}),
        json!({ "type": "manual" }),
        now,
    )
    .await
    .unwrap()
    .id;

    let worker = bia()
        .args(["serve", "--role", "worker"])
        .env("BIA_DATABASE_URL", &url)
        .env("BIA_LOG_FORMAT", "json")
        .env("BIA_WORKER_POLL_INTERVAL", "100ms")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A real process on the system Clock, so this waits in real time.
    let succeeded = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let status = PgRunRepository.get(&mut conn, run).await.unwrap().unwrap();
            if status.status == RunStatus::Succeeded {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let terminated = std::process::Command::new("kill")
        .args(["-TERM", &worker.id().unwrap().to_string()])
        .status()
        .unwrap();
    assert!(terminated.success());
    let stopped = tokio::time::timeout(Duration::from_secs(60), worker.wait_with_output())
        .await
        .expect("bia did not stop on SIGTERM")
        .unwrap();

    let logs = String::from_utf8_lossy(&stopped.stdout);
    assert!(succeeded.is_ok(), "the Run did not succeed; logs:\n{logs}");
    assert!(stopped.status.success(), "{stopped:?}");
    let entries = PgTimelineRepository.list(&mut conn, run).await.unwrap();
    assert_eq!(entries[0].output, Some(json!(42.0)));
    assert!(
        logs.contains(&run.to_string()),
        "logs carry the Run id:\n{logs}"
    );
    assert!(logs.contains("worker stopped"), "{logs}");
}

#[tokio::test]
async fn invalid_configuration_fails_at_startup_with_a_clear_message() {
    let refused = finished(
        bia()
            .args(["serve", "--role", "worker"])
            .env("BIA_DATABASE_URL", "postgres://bia:bia@localhost:5432/bia")
            .env("BIA_WORKER_CONCURRENCY", "lots"),
    )
    .await;

    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("`worker.concurrency` in BIA_WORKER_CONCURRENCY"),
        "{stderr}"
    );
}

#[tokio::test]
async fn a_malformed_master_key_is_refused_without_echoing_it() {
    let refused = finished(
        bia()
            .args(["serve", "--role", "worker"])
            .env("BIA_DATABASE_URL", "postgres://bia:bia@localhost:5432/bia")
            .env("BIA_MASTER_KEY", "not-hex-but-secret"),
    )
    .await;

    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("`master_key`"), "{stderr}");
    assert!(!stderr.contains("not-hex-but-secret"), "{stderr}");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn serve_refuses_to_start_when_secrets_exist_but_there_is_no_master_key(pool: PgPool) {
    sqlx::query(
        "INSERT INTO secrets (name, ciphertext, nonce, created_at, updated_at) \
         VALUES ('API_TOKEN', '\\x00', decode(repeat('00', 24), 'hex'), now(), now())",
    )
    .execute(&pool)
    .await
    .unwrap();

    let refused = finished(
        bia()
            .args(["serve", "--role", "worker"])
            .env("BIA_DATABASE_URL", database_url(&pool)),
    )
    .await;

    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("BIA_MASTER_KEY"), "{stderr}");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn bia_serve_as_a_scheduler_leads_and_stops_on_sigterm(pool: PgPool) {
    let mut scheduler = bia()
        .args(["serve", "--role", "scheduler"])
        .env("BIA_DATABASE_URL", database_url(&pool))
        .env("BIA_LOG_FORMAT", "json")
        .env("BIA_SCHEDULER_POLL_INTERVAL", "100ms")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(scheduler.stdout.take().unwrap()).lines();
    let mut logs = String::new();
    let leading = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(line) = stdout.next_line().await.unwrap() {
            logs.push_str(&line);
            logs.push('\n');
            if line.contains("\"leading\"") {
                return;
            }
        }
    })
    .await;
    assert!(leading.is_ok(), "the scheduler did not lead; logs:\n{logs}");

    let terminated = std::process::Command::new("kill")
        .args(["-TERM", &scheduler.id().unwrap().to_string()])
        .status()
        .unwrap();
    assert!(terminated.success());
    while let Some(line) = stdout.next_line().await.unwrap() {
        logs.push_str(&line);
        logs.push('\n');
    }
    let stopped = tokio::time::timeout(Duration::from_secs(60), scheduler.wait())
        .await
        .expect("bia did not stop on SIGTERM")
        .unwrap();
    assert!(stopped.success(), "{stopped:?}");
    assert!(logs.contains("scheduler stopped"), "{logs}");
}

const PASSWORD: &str = "cli-test-password";

/// A running `bia serve --role api` and the JSON logs it has written so far.
struct Api {
    process: tokio::process::Child,
    stdout: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    logs: String,
}

impl Api {
    async fn start(pool: &PgPool, extra: &[&str]) -> Self {
        let mut process = bia()
            .args(["serve", "--role", "api"])
            .args(extra)
            .env("BIA_DATABASE_URL", database_url(pool))
            .env("BIA_LOG_FORMAT", "json")
            .env("BIA_LOG_FILTER", "debug")
            .env("BIA_API_BIND", "127.0.0.1:0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = BufReader::new(process.stdout.take().unwrap()).lines();
        Self {
            process,
            stdout,
            logs: String::new(),
        }
    }

    /// Reads logs until an entry has `field`, and returns its value.
    async fn field(&mut self, field: &str) -> String {
        let found = tokio::time::timeout(Duration::from_secs(60), async {
            while let Some(line) = self.stdout.next_line().await.unwrap() {
                self.logs.push_str(&line);
                self.logs.push('\n');
                let entry: serde_json::Value = serde_json::from_str(&line).unwrap();
                if let Some(value) = entry["fields"][field].as_str() {
                    return Some(value.to_owned());
                }
            }
            None
        })
        .await;
        match found {
            Ok(Some(value)) => value,
            _ => panic!("no log entry has `{field}`; logs:\n{}", self.logs),
        }
    }

    /// Stops it with SIGTERM and returns every log line.
    async fn stop(mut self) -> String {
        let terminated = std::process::Command::new("kill")
            .args(["-TERM", &self.process.id().unwrap().to_string()])
            .status()
            .unwrap();
        assert!(terminated.success());
        while let Some(line) = self.stdout.next_line().await.unwrap() {
            self.logs.push_str(&line);
            self.logs.push('\n');
        }
        let stopped = tokio::time::timeout(Duration::from_secs(60), self.process.wait())
            .await
            .expect("bia did not stop on SIGTERM")
            .unwrap();
        assert!(stopped.success(), "{stopped:?}");
        self.logs
    }
}

/// Runs `bia setup` against `url`, piping `input` as the code and password lines.
async fn bia_setup(url: &str, input: &str) -> Output {
    let mut setup = bia()
        .arg("setup")
        .env("BIA_URL", url)
        .env_remove("BIA_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = setup.stdin.take().unwrap();
    stdin.write_all(input.as_bytes()).await.unwrap();
    drop(stdin);
    tokio::time::timeout(Duration::from_secs(60), setup.wait_with_output())
        .await
        .expect("bia setup did not exit")
        .unwrap()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn bia_serve_logs_a_setup_link_that_bia_setup_uses_then_serves_and_stops_on_sigterm(
    pool: PgPool,
) {
    let mut api = Api::start(&pool, &[]).await;
    let address = api.field("address").await;
    let link = api.field("setup_link").await;
    let (_, code) = link.split_once("/setup?code=").unwrap();
    let code = code.to_owned();
    let port = address.rsplit_once(':').unwrap().1;
    assert_eq!(link, format!("http://localhost:{port}/setup?code={code}"));

    let url = format!("http://{address}");
    let short = bia_setup(&url, &format!("{code}\nshort\n")).await;
    assert!(!short.status.success(), "{short:?}");
    let stderr = String::from_utf8_lossy(&short.stderr);
    assert!(stderr.contains("password_too_short"), "{stderr}");

    let set_up = bia_setup(&url, &format!("{link}\n{PASSWORD}\n")).await;
    assert!(set_up.status.success(), "{set_up:?}");
    let token = String::from_utf8(set_up.stdout).unwrap();
    let token = token.trim().to_owned();
    assert!(token.starts_with("bia_"), "{token}");
    let again = bia_setup(&url, &format!("{code}\n{PASSWORD}\n")).await;
    assert!(!again.status.success(), "{again:?}");
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert!(stderr.contains("already_set_up"), "{stderr}");

    let client = bia_client::Client::new(&url).unwrap();
    assert_eq!(client.ready().await.unwrap().status, "ready");
    assert!(client.schema().await.is_err(), "the schema needs a Token");
    let schema = client.with_token(&token).schema().await.unwrap();
    assert!(
        schema
            .actions
            .iter()
            .any(|action| action.name == "http.request"),
        "the builtin Actions are listed"
    );

    let logs = api.stop().await;
    assert!(logs.contains("api stopped"), "{logs}");
    assert!(logs.contains("setup done"), "{logs}");
    assert_eq!(
        logs.matches(&code).count(),
        1,
        "the code is logged beyond the Setup Link:\n{logs}"
    );
    assert!(!logs.contains(&token), "the Token is logged:\n{logs}");
    assert!(!logs.contains(PASSWORD), "the password is logged:\n{logs}");

    // A restart without a reset logs no link, since the User can sign in.
    let mut api = Api::start(&pool, &[]).await;
    api.field("address").await;
    let logs = api.stop().await;
    assert!(!logs.contains("setup_link"), "{logs}");

    let mut api = Api::start(&pool, &["--reset-admin"]).await;
    let address = api.field("address").await;
    let new_link = api.field("setup_link").await;
    let url = format!("http://{address}");
    let old = bia_client::Client::new(&url).unwrap().with_token(&token);
    assert!(old.schema().await.is_err(), "the reset revoked the Token");
    let stale = bia_setup(&url, &format!("{code}\n{PASSWORD}\n")).await;
    let stderr = String::from_utf8_lossy(&stale.stderr);
    assert!(stderr.contains("setup_code_expired"), "{stderr}");
    let set_up = bia_setup(&url, &format!("{new_link}\nanother-password\n")).await;
    assert!(set_up.status.success(), "{set_up:?}");
    let logs = api.stop().await;
    assert!(logs.contains("admin reset"), "{logs}");
    assert!(logs.contains(r#""revoked_tokens":1"#), "{logs}");
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn reset_admin_needs_the_api_role(pool: PgPool) {
    let refused = finished(
        bia()
            .args(["serve", "--role", "worker", "--reset-admin"])
            .env("BIA_DATABASE_URL", database_url(&pool)),
    )
    .await;

    assert!(!refused.status.success(), "{refused:?}");
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("--reset-admin needs the `api` Role"),
        "{stderr}"
    );
}
