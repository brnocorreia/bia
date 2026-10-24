//! The `bia` binary migrates an empty database, runs a Run to completion as a Worker, leads as a
//! scheduler, serves the API, stops on SIGTERM, and refuses to start with invalid
//! configuration.

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
use tokio::io::{AsyncBufReadExt, BufReader};
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

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn bia_serve_as_the_api_serves_the_schema_and_stops_on_sigterm(pool: PgPool) {
    let mut api = bia()
        .args(["serve", "--role", "api"])
        .env("BIA_DATABASE_URL", database_url(&pool))
        .env("BIA_LOG_FORMAT", "json")
        .env("BIA_API_BIND", "127.0.0.1:0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(api.stdout.take().unwrap()).lines();
    let mut logs = String::new();
    let listening = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(line) = stdout.next_line().await.unwrap() {
            logs.push_str(&line);
            logs.push('\n');
            let entry: serde_json::Value = serde_json::from_str(&line).unwrap();
            if entry["fields"]["message"] == "api listening" {
                return entry["fields"]["address"].as_str().unwrap().to_owned();
            }
        }
        panic!("bia exited before listening");
    })
    .await;
    let address = listening.unwrap_or_else(|_| panic!("the api did not listen; logs:\n{logs}"));

    let client = bia_client::Client::new(&format!("http://{address}")).unwrap();
    assert_eq!(client.ready().await.unwrap().status, "ready");
    let schema = client.schema().await.unwrap();
    assert!(
        schema
            .actions
            .iter()
            .any(|action| action.name == "http.request"),
        "the builtin Actions are listed"
    );

    let terminated = std::process::Command::new("kill")
        .args(["-TERM", &api.id().unwrap().to_string()])
        .status()
        .unwrap();
    assert!(terminated.success());
    while let Some(line) = stdout.next_line().await.unwrap() {
        logs.push_str(&line);
        logs.push('\n');
    }
    let stopped = tokio::time::timeout(Duration::from_secs(60), api.wait())
        .await
        .expect("bia did not stop on SIGTERM")
        .unwrap();
    assert!(stopped.success(), "{stopped:?}");
    assert!(logs.contains("api stopped"), "{logs}");
}
