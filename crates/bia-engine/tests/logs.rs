//! Every log line about a Run's Steps carries the Run's id.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex};

use bia_core::action::testing::FakeAction;
use bia_store::run::RunStatus;
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

use common::{Harness, config};

/// Collects what a `tracing` subscriber writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn logs_for_a_runs_steps_carry_its_run_id(pool: PgPool) {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .finish();
    // Global rather than scoped to this thread: a scoped subscriber misses events from
    // callsites whose interest another test thread cached first. This file holds one test,
    // so nothing else logs into it.
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let echo = Arc::new(FakeAction::new("test.echo"));
    let mut h = Harness::new(pool, vec![echo]);
    h.publish(
        "greet",
        json!([
            { "id": "hello", "action": "test.echo" },
            { "id": "bye", "action": "test.echo" },
        ]),
    )
    .await;
    let run = h.start_run().await;

    let worker = h.worker("worker-a", config());
    h.wait_for(run, RunStatus::Succeeded).await;
    worker.shut_down().await;

    let output = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let step_lines: Vec<&Value> = lines
        .iter()
        .filter(|line| line["fields"]["step_id"].is_string())
        .collect();
    let steps: BTreeSet<&str> = step_lines
        .iter()
        .map(|line| line["fields"]["step_id"].as_str().unwrap())
        .collect();
    assert_eq!(steps, BTreeSet::from(["hello", "bye"]));
    for line in step_lines {
        let run_ids: Vec<&Value> = line["spans"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|span| span.get("run_id"))
            .collect();
        assert_eq!(run_ids, [&json!(run.to_string())], "{line}");
    }
}
