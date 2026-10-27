//! Queue Triggers start Runs of the Active Version from batches of a Queue's Messages, which
//! each Run holds until it finishes: acked when it succeeds, delivered again when it fails
//! until the Queue's retries run out, dead-lettered when it is cancelled (ADR-0061).

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use bia_client::{MessageQuery, QueueSettings, RunQuery, RunStatus, RunSummary};
use bia_core::clock::Clock;
use bia_engine::{Scheduler, SchedulerConfig};
use chrono::TimeDelta;
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

use common::{TestServer, Workers, eventually, spawn_worker};

const QUEUE: &str = "signups";
const DLQ: &str = "signups-dlq";
const CONSUMER: &str = "onboard";

struct Harness {
    server: TestServer,
    scheduler: Scheduler,
    _workers: Workers,
}

impl Harness {
    /// A server with a Worker and a scheduler, `signups` dead-lettering into `signups-dlq`
    /// after `max_retries`, and a `publisher` Workflow that publishes `input.bodies` to it.
    async fn start(pool: PgPool, max_retries: u32) -> Self {
        let server = TestServer::start(pool.clone()).await;
        let workers = spawn_worker(&pool, &server.clock, Arc::new(common::registry()));
        let scheduler = Scheduler::new(
            "test-scheduler",
            pool,
            Arc::new(server.clock.clone()),
            SchedulerConfig::default(),
        );
        let client = &server.client;
        client
            .put_queue(DLQ, &QueueSettings::default())
            .await
            .unwrap();
        let settings = QueueSettings {
            max_retries: Some(max_retries),
            visibility_timeout: Some("30s".into()),
            batch_size: None,
            dead_letter: Some(DLQ.into()),
        };
        client.put_queue(QUEUE, &settings).await.unwrap();
        let harness = Self {
            server,
            scheduler,
            _workers: workers,
        };
        harness
            .deploy(&json!({
                "schema_version": 1,
                "id": "publisher",
                "triggers": [{ "id": "by_hand", "type": "manual" }],
                "steps": [{
                    "id": "publish",
                    "action": "queue.publish_batch",
                    "params": { "queue": QUEUE, "bodies": "{{ input.bodies }}" },
                }],
            }))
            .await;
        harness
    }

    /// Publishes and activates `definition` as its Workflow's next Version.
    async fn deploy(&self, definition: &Value) -> i32 {
        let client = &self.server.client;
        let id = definition["id"].as_str().unwrap();
        let number = client
            .create_draft(id, definition)
            .await
            .unwrap()
            .version
            .summary
            .number;
        client.publish_version(id, number).await.unwrap();
        client.activate_version(id, number).await.unwrap();
        number
    }

    /// Activates `steps` as the next Version of the consumer Workflow, with `triggers`.
    async fn consumer(&self, triggers: Value, steps: Value) -> i32 {
        self.deploy(&json!({
            "schema_version": 1,
            "id": CONSUMER,
            "triggers": triggers,
            "steps": steps,
        }))
        .await
    }

    /// Publishes `{ "n": 0 }`, `{ "n": 1 }`, … to `signups` through the `publisher` Workflow.
    async fn publish(&self, count: usize) {
        let bodies: Vec<Value> = (0..count).map(|n| json!({ "n": n })).collect();
        let run = self
            .server
            .client
            .trigger_run("publisher", &json!({ "bodies": bodies }), None)
            .await
            .unwrap();
        assert_eq!(self.finished(run.id).await, RunStatus::Succeeded);
    }

    async fn tick(&self) -> usize {
        self.scheduler.tick().await.unwrap().len()
    }

    async fn finished(&self, run: uuid::Uuid) -> RunStatus {
        let client = &self.server.client;
        eventually("the run to finish", || async {
            client
                .get_run(run)
                .await
                .unwrap()
                .summary
                .status
                .is_finished()
        })
        .await;
        client.get_run(run).await.unwrap().summary.status
    }

    async fn wait_for(&self, run: uuid::Uuid, status: RunStatus) {
        let client = &self.server.client;
        eventually("the run to reach its status", || async {
            client.get_run(run).await.unwrap().summary.status == status
        })
        .await;
    }

    /// The consumer's Runs, by their first Message, then oldest first.
    async fn runs(&self) -> Vec<RunSummary> {
        let mut runs = self
            .server
            .client
            .list_runs(&RunQuery {
                workflow_id: Some(CONSUMER.into()),
                ..RunQuery::default()
            })
            .await
            .unwrap()
            .runs;
        runs.sort_by_key(|run| (run.trigger["messages"][0]["id"].as_i64(), run.created_at));
        runs
    }

    async fn messages(&self, queue: &str) -> Vec<bia_client::Message> {
        self.server
            .client
            .list_messages(queue, &MessageQuery::default())
            .await
            .unwrap()
            .messages
    }
}

fn queue_trigger(batch_size: u32) -> Value {
    json!([{ "id": "new_signup", "type": "queue", "queue": QUEUE, "batch_size": batch_size }])
}

fn succeed() -> Value {
    json!([{ "id": "done", "action": "flow.succeed" }])
}

/// The `n` of each body in a Run's `input.messages`.
fn numbers(input: &Value) -> Vec<i64> {
    input["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|body| body["n"].as_i64().unwrap())
        .collect()
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn twenty_five_messages_in_batches_of_ten_start_runs_of_ten_ten_and_five(pool: PgPool) {
    let harness = Harness::start(pool, 3).await;
    let version = harness.consumer(queue_trigger(10), succeed()).await;
    harness.publish(25).await;

    assert_eq!(harness.tick().await, 3);

    let runs = harness.runs().await;
    let mut inputs = Vec::new();
    for run in &runs {
        assert_eq!(run.version, version);
        let input = harness.server.client.get_run(run.id).await.unwrap().input;
        let trigger = &run.trigger;
        assert_eq!(trigger["type"], "queue");
        assert_eq!(trigger["id"], "new_signup");
        assert_eq!(trigger["queue"], QUEUE);
        let metadata = trigger["messages"].as_array().unwrap();
        assert_eq!(metadata.len(), numbers(&input).len());
        assert!(metadata.iter().all(|message| message["deliveries"] == 1));
        assert!(
            metadata
                .iter()
                .all(|message| message["published_at"] == json!(common::t0()))
        );
        inputs.push(numbers(&input));
    }
    assert_eq!(
        inputs,
        [
            (0..10).collect::<Vec<_>>(),
            (10..20).collect(),
            (20..25).collect(),
        ]
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_run_that_succeeds_acks_its_messages_and_they_are_never_delivered_again(pool: PgPool) {
    let harness = Harness::start(pool, 3).await;
    harness.consumer(queue_trigger(10), succeed()).await;
    harness.publish(3).await;

    assert_eq!(harness.tick().await, 1);
    let run = harness.runs().await.remove(0);
    assert_eq!(harness.finished(run.id).await, RunStatus::Succeeded);

    assert!(harness.messages(QUEUE).await.is_empty());
    assert!(harness.messages(DLQ).await.is_empty());
    harness.server.clock.advance(TimeDelta::hours(1));
    assert_eq!(harness.tick().await, 0);
    assert_eq!(harness.runs().await.len(), 1);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_run_that_fails_returns_its_messages_until_their_retries_run_out(pool: PgPool) {
    let harness = Harness::start(pool, 1).await;
    let fail =
        json!([{ "id": "boom", "action": "flow.fail", "params": { "message": "bad signup" } }]);
    harness.consumer(queue_trigger(10), fail).await;
    harness.publish(2).await;

    // First delivery: the Run fails, and the Messages wait out a 1s backoff.
    assert_eq!(harness.tick().await, 1);
    let first = harness.runs().await.remove(0);
    assert_eq!(harness.finished(first.id).await, RunStatus::Failed);
    let waiting = harness.messages(QUEUE).await;
    assert_eq!(waiting.len(), 2);
    for message in &waiting {
        assert_eq!(message.deliveries, 1);
        assert!(!message.in_flight);
        assert_eq!(
            message.visible_at,
            Some(harness.server.clock.now() + TimeDelta::seconds(1))
        );
        let error = message.last_error.as_deref().unwrap();
        assert!(error.contains(&first.id.to_string()), "{error}");
        assert!(error.contains("bad signup"), "{error}");
    }
    assert_eq!(harness.tick().await, 0);

    // The one retry `max_retries: 1` allows: a second Run with the same Messages.
    harness.server.clock.advance(TimeDelta::seconds(1));
    assert_eq!(harness.tick().await, 1);
    let second = harness.runs().await.remove(1);
    assert_eq!(second.trigger["messages"][0]["deliveries"], 2);
    let input = harness
        .server
        .client
        .get_run(second.id)
        .await
        .unwrap()
        .input;
    assert_eq!(numbers(&input), [0, 1]);
    assert_eq!(harness.finished(second.id).await, RunStatus::Failed);

    // Out of retries: dead-lettered, and no further Run.
    assert!(harness.messages(QUEUE).await.is_empty());
    let dead = harness.messages(DLQ).await;
    assert_eq!(dead.len(), 2);
    assert!(dead.iter().all(|message| {
        message.dead_lettered_from.as_deref() == Some(QUEUE)
            && message
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains(&second.id.to_string()))
    }));
    harness.server.clock.advance(TimeDelta::hours(2));
    assert_eq!(harness.tick().await, 0);
    assert_eq!(harness.runs().await.len(), 2);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_run_holds_its_messages_past_the_visibility_timeout_and_cancelling_dead_letters_them(
    pool: PgPool,
) {
    let harness = Harness::start(pool, 3).await;
    let sleep = json!([{ "id": "nap", "action": "flow.sleep", "params": { "duration": "1h" } }]);
    harness.consumer(queue_trigger(10), sleep).await;
    harness.publish(2).await;

    assert_eq!(harness.tick().await, 1);
    let run = harness.runs().await.remove(0);
    harness.wait_for(run.id, RunStatus::Sleeping).await;

    harness.server.clock.advance(TimeDelta::minutes(5));
    assert_eq!(harness.tick().await, 0);
    let held = harness.messages(QUEUE).await;
    assert_eq!(held.len(), 2);
    assert!(held.iter().all(|message| message.in_flight));
    let counts = harness
        .server
        .client
        .get_queue(QUEUE)
        .await
        .unwrap()
        .messages;
    assert_eq!((counts.ready, counts.in_flight), (0, 2));

    harness.server.client.cancel_run(run.id).await.unwrap();
    assert_eq!(harness.finished(run.id).await, RunStatus::Cancelled);
    assert!(harness.messages(QUEUE).await.is_empty());
    let dead = harness.messages(DLQ).await;
    assert_eq!(dead.len(), 2);
    let cancelled = format!("Run {} was cancelled", run.id);
    assert!(
        dead.iter()
            .all(|message| message.last_error.as_deref() == Some(cancelled.as_str()))
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn only_the_active_version_consumes_and_dropping_the_trigger_stops_it(pool: PgPool) {
    let harness = Harness::start(pool, 3).await;
    let with_trigger = harness.consumer(queue_trigger(10), succeed()).await;
    harness
        .consumer(json!([{ "id": "by_hand", "type": "manual" }]), succeed())
        .await;
    harness.publish(3).await;

    assert_eq!(harness.tick().await, 0);
    harness.server.clock.advance(TimeDelta::minutes(1));
    assert_eq!(harness.tick().await, 0);
    let waiting = harness.messages(QUEUE).await;
    assert_eq!(waiting.len(), 3);
    assert!(waiting.iter().all(|message| message.deliveries == 0));

    harness
        .server
        .client
        .activate_version(CONSUMER, with_trigger)
        .await
        .unwrap();
    assert_eq!(harness.tick().await, 1);
    assert_eq!(harness.runs().await[0].version, with_trigger);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn messages_that_break_the_input_schema_fail_their_run_and_end_dead_lettered(pool: PgPool) {
    let harness = Harness::start(pool, 0).await;
    harness
        .deploy(&json!({
            "schema_version": 1,
            "id": CONSUMER,
            "triggers": queue_trigger(10),
            "input_schema": {
                "type": "object",
                "properties": {
                    "messages": { "type": "array", "items": { "required": ["email"] } },
                },
            },
            "steps": succeed(),
        }))
        .await;
    harness.publish(1).await;

    assert_eq!(harness.tick().await, 1);
    let run = harness.runs().await.remove(0);
    assert_eq!(harness.finished(run.id).await, RunStatus::Failed);

    assert!(harness.messages(QUEUE).await.is_empty());
    let dead = harness.messages(DLQ).await;
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].body, json!({ "n": 0 }));
}
