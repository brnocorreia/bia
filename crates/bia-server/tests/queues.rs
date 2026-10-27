//! Queues through the API: created, replaced and deleted by `admin`, read with their Messages
//! by `agent`, checked by validation, and published to by Workflows (ADR-0015, ADR-0058).

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use bia_client::{Error, MessageQuery, QueueSettings, RunStatus, Scope, Target, TimelineEntry};
use bia_core::clock::Clock;
use bia_core::dsl::QueueName;
use bia_core::validation::Severity;
use bia_store::job::Backoff;
use bia_store::queue::{PgQueueRepository, QueueRepository};
use chrono::TimeDelta;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

mod common;

use common::{TestServer, Workers, eventually, spawn_worker};

fn failure<T: std::fmt::Debug>(result: Result<T, Error>) -> (StatusCode, String) {
    match result {
        Err(Error::Api { status, body }) => (status, body.code),
        other => panic!("expected an API error, got {other:?}"),
    }
}

fn settings(dead_letter: Option<&str>, max_retries: u32) -> QueueSettings {
    QueueSettings {
        max_retries: Some(max_retries),
        visibility_timeout: Some("30s".into()),
        batch_size: None,
        dead_letter: dead_letter.map(str::to_owned),
    }
}

fn publisher(steps: Value) -> Value {
    json!({
        "schema_version": 1,
        "id": "publisher",
        "triggers": [{ "id": "by_hand", "type": "manual" }],
        "steps": steps,
    })
}

/// A server with a Worker, `emails` dead-lettering into `emails-dlq`, and `steps` as the
/// Active Version of `publisher`.
async fn publishing(pool: PgPool, steps: Value) -> (TestServer, Workers) {
    let server = TestServer::start(pool.clone()).await;
    let workers = spawn_worker(&pool, &server.clock, Arc::new(common::registry()));
    let client = &server.client;
    client
        .put_queue("emails-dlq", &QueueSettings::default())
        .await
        .unwrap();
    client
        .put_queue("emails", &settings(Some("emails-dlq"), 2))
        .await
        .unwrap();
    client
        .create_draft("publisher", &publisher(steps))
        .await
        .unwrap();
    client.publish_version("publisher", 1).await.unwrap();
    client.activate_version("publisher", 1).await.unwrap();
    (server, workers)
}

async fn finished(server: &TestServer, run: Uuid) -> (RunStatus, Vec<TimelineEntry>) {
    eventually("the run to finish", || async {
        server
            .client
            .get_run(run)
            .await
            .unwrap()
            .summary
            .status
            .is_finished()
    })
    .await;
    let run = server.client.get_run(run).await.unwrap();
    (run.summary.status, run.timeline)
}

async fn messages(server: &TestServer, queue: &str) -> Vec<bia_client::Message> {
    server
        .client
        .list_messages(queue, &MessageQuery::default())
        .await
        .unwrap()
        .messages
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn admins_put_and_delete_queues_and_agents_read_them(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let admin = &server.client;
    let agent = server.client_with(&server.token(Scope::Agent).await);

    let defaults = admin
        .put_queue("emails-dlq", &QueueSettings::default())
        .await
        .unwrap();
    assert_eq!(
        (defaults.max_retries, defaults.visibility_timeout.as_str()),
        (3, "30s")
    );
    assert_eq!((defaults.batch_size, defaults.dead_letter), (10, None));
    let emails = QueueSettings {
        max_retries: Some(5),
        visibility_timeout: Some("90s".into()),
        batch_size: Some(25),
        dead_letter: Some("emails-dlq".into()),
    };
    let created = admin.put_queue("emails", &emails).await.unwrap();
    assert_eq!(created.visibility_timeout, "90s");
    assert_eq!(created.created_at, common::t0());

    server.clock.advance(TimeDelta::minutes(1));
    let replaced = admin.put_queue("emails", &settings(None, 1)).await.unwrap();
    assert_eq!((replaced.max_retries, replaced.batch_size), (1, 10));
    assert_eq!(replaced.dead_letter, None);
    assert_eq!(replaced.created_at, common::t0());
    assert!(replaced.updated_at > replaced.created_at);

    let names: Vec<_> = agent
        .list_queues()
        .await
        .unwrap()
        .into_iter()
        .map(|queue| queue.name)
        .collect();
    assert_eq!(names, ["emails", "emails-dlq"]);
    assert_eq!(agent.get_queue("emails").await.unwrap(), replaced);
    assert_eq!(
        failure(agent.put_queue("other", &QueueSettings::default()).await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );
    assert_eq!(
        failure(agent.delete_queue("emails").await),
        (StatusCode::FORBIDDEN, "insufficient_scope".into())
    );

    admin.delete_queue("emails").await.unwrap();
    assert_eq!(
        failure(agent.get_queue("emails").await),
        (StatusCode::NOT_FOUND, "queue_not_found".into())
    );
    assert_eq!(
        failure(admin.delete_queue("emails").await),
        (StatusCode::NOT_FOUND, "queue_not_found".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn queue_settings_and_dead_letter_queues_are_checked(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let client = &server.client;

    assert_eq!(
        failure(
            client
                .put_queue("emails", &settings(Some("emails-dlq"), 3))
                .await
        ),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            "dead_letter_queue_not_found".into()
        )
    );
    for bad in [
        settings(Some("emails"), 3),
        QueueSettings {
            batch_size: Some(101),
            ..QueueSettings::default()
        },
        QueueSettings {
            visibility_timeout: Some("0s".into()),
            ..QueueSettings::default()
        },
    ] {
        assert_eq!(
            failure(client.put_queue("emails", &bad).await),
            (StatusCode::BAD_REQUEST, "malformed_request".into()),
            "{bad:?}"
        );
    }
    let unparsable = QueueSettings {
        visibility_timeout: Some("1h30m".into()),
        ..QueueSettings::default()
    };
    assert_eq!(
        failure(client.put_queue("emails", &unparsable).await),
        (StatusCode::UNPROCESSABLE_ENTITY, "malformed_request".into())
    );
    assert_eq!(
        failure(client.put_queue("Emails", &QueueSettings::default()).await),
        (StatusCode::BAD_REQUEST, "malformed_request".into())
    );

    client
        .put_queue("emails-dlq", &QueueSettings::default())
        .await
        .unwrap();
    client
        .put_queue("emails", &settings(Some("emails-dlq"), 3))
        .await
        .unwrap();
    assert_eq!(
        failure(client.delete_queue("emails-dlq").await),
        (StatusCode::CONFLICT, "queue_in_use".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_unknown_queue_is_a_warning_on_a_draft_and_an_error_at_publish(pool: PgPool) {
    let server = TestServer::start(pool).await;
    let client = &server.client;
    client
        .put_queue("emails", &QueueSettings::default())
        .await
        .unwrap();
    let definition = publisher(json!([{
        "id": "send",
        "action": "queue.publish",
        "params": { "queue": "emals", "body": { "to": "{{ input.email }}" } },
    }]));

    let saved = client.create_draft("publisher", &definition).await.unwrap();
    let warning = &saved.diagnostics[0];
    assert_eq!(saved.diagnostics.len(), 1, "{:?}", saved.diagnostics);
    assert_eq!(warning.code.as_str(), "unknown_queue");
    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(warning.path, "/steps/0/params/queue");
    assert_eq!(
        warning.suggestion.as_deref(),
        Some("Did you mean 'emails'?")
    );

    let at_publish = client
        .validate_version("publisher", 1, Target::Publish)
        .await
        .unwrap();
    assert!(!at_publish.valid);
    let Err(Error::Api { status, body }) = client.publish_version("publisher", 1).await else {
        panic!("publishing should fail");
    };
    assert_eq!(
        (status, body.code.as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_definition")
    );
    assert_eq!(body.diagnostics[0].code.as_str(), "unknown_queue");
    assert_eq!(body.diagnostics[0].severity, Severity::Error);

    client
        .put_queue("emals", &QueueSettings::default())
        .await
        .unwrap();
    client.publish_version("publisher", 1).await.unwrap();
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_publish_step_enqueues_a_message_that_a_delay_keeps_hidden(pool: PgPool) {
    let steps = json!([
        {
            "id": "now",
            "action": "queue.publish",
            "params": { "queue": "emails", "body": { "to": "{{ input.email }}" } },
        },
        {
            "id": "later",
            "action": "queue.publish_batch",
            "params": { "queue": "emails", "bodies": [1, 2], "delay": "10m" },
        },
    ]);
    let (server, _workers) = publishing(pool.clone(), steps).await;
    let started = server
        .client
        .trigger_run("publisher", &json!({ "email": "ada@example.com" }), None)
        .await
        .unwrap();

    let (status, timeline) = finished(&server, started.id).await;
    assert_eq!(status, RunStatus::Succeeded);
    let t0 = common::t0();
    assert_eq!(
        timeline[0].output,
        Some(
            json!({ "queue": "emails", "published": 1, "visible_at": t0.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true) })
        )
    );
    let later = t0 + TimeDelta::minutes(10);
    assert_eq!(timeline[1].output.as_ref().unwrap()["published"], 2);

    let published = messages(&server, "emails").await;
    let bodies: Vec<_> = published.iter().map(|m| m.body.clone()).collect();
    assert_eq!(
        bodies,
        [json!({ "to": "ada@example.com" }), json!(1), json!(2)]
    );
    assert_eq!(published[1].visible_at, Some(later));

    let queues = PgQueueRepository;
    let emails = QueueName::new("emails").unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let claim = |now| {
        let emails = emails.clone();
        let pool = pool.clone();
        async move {
            let mut conn = pool.acquire().await.unwrap();
            queues
                .claim(&mut conn, &emails, "consumer", 10, now)
                .await
                .unwrap()
        }
    };
    let first = claim(server.clock.now()).await;
    assert_eq!(first.len(), 1, "only the undelayed Message is claimable");
    queues.ack(&mut conn, &first[0].lease).await.unwrap();
    assert!(claim(later - TimeDelta::milliseconds(1)).await.is_empty());
    assert_eq!(claim(later).await.len(), 2);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_unacked_message_reappears_and_ends_in_the_dead_letter_queue(pool: PgPool) {
    let steps = json!([{
        "id": "send",
        "action": "queue.publish",
        "params": { "queue": "emails", "body": "hello" },
    }]);
    let (server, _workers) = publishing(pool.clone(), steps).await;
    let started = server
        .client
        .trigger_run("publisher", &json!({}), None)
        .await
        .unwrap();
    finished(&server, started.id).await;

    let queues = PgQueueRepository;
    let emails = QueueName::new("emails").unwrap();
    let mut conn = pool.acquire().await.unwrap();
    // `emails` gives a Message its first delivery and two retries, each hidden for 30s.
    for delivery in 1..=3 {
        let claimed = queues
            .claim(&mut conn, &emails, "consumer", 10, server.clock.now())
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1, "delivery {delivery}");
        let listed = messages(&server, "emails").await;
        assert_eq!(
            (listed[0].deliveries, listed[0].in_flight),
            (delivery, true)
        );
        server.clock.advance(TimeDelta::seconds(30));
    }
    let claimed = queues
        .claim(&mut conn, &emails, "consumer", 10, server.clock.now())
        .await
        .unwrap();
    assert!(claimed.is_empty());

    assert!(messages(&server, "emails").await.is_empty());
    let dead = server
        .client
        .list_messages(
            "emails-dlq",
            &MessageQuery {
                dead_letters: Some(true),
                ..MessageQuery::default()
            },
        )
        .await
        .unwrap()
        .messages;
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].body, json!("hello"));
    assert_eq!(dead[0].dead_lettered_from.as_deref(), Some("emails"));
    assert_eq!(dead[0].dead_lettered_at, Some(server.clock.now()));
    assert!(!dead[0].in_flight);
    assert!(
        dead[0]
            .last_error
            .as_deref()
            .unwrap()
            .contains("visibility timeout")
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_nacked_message_is_retried_then_dead_lettered(pool: PgPool) {
    let server = TestServer::start(pool.clone()).await;
    server
        .client
        .put_queue("audit", &settings(None, 0))
        .await
        .unwrap();
    let queues = PgQueueRepository;
    let audit = QueueName::new("audit").unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let now = server.clock.now();
    queues
        .publish(&mut conn, &audit, &[json!(1)], now, now)
        .await
        .unwrap();
    let claimed = queues
        .claim(&mut conn, &audit, "consumer", 10, now)
        .await
        .unwrap();
    queues
        .nack(
            &mut conn,
            &claimed[0].lease,
            "rejected",
            &Backoff::default(),
            now,
        )
        .await
        .unwrap();

    let kept = messages(&server, "audit").await;
    assert_eq!(kept[0].visible_at, None, "it is never delivered again");
    assert_eq!(kept[0].last_error.as_deref(), Some("rejected"));
    assert_eq!(kept[0].dead_lettered_from.as_deref(), Some("audit"));
    let pending = server
        .client
        .list_messages(
            "audit",
            &MessageQuery {
                dead_letters: Some(false),
                ..MessageQuery::default()
            },
        )
        .await
        .unwrap();
    assert!(pending.messages.is_empty());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn messages_are_paged_oldest_first(pool: PgPool) {
    let server = TestServer::start(pool.clone()).await;
    server
        .client
        .put_queue("emails", &QueueSettings::default())
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let bodies: Vec<_> = (0..5).map(|n| json!(n)).collect();
    let now = server.clock.now();
    PgQueueRepository
        .publish(
            &mut conn,
            &QueueName::new("emails").unwrap(),
            &bodies,
            now,
            now,
        )
        .await
        .unwrap();

    let page = |after| MessageQuery {
        after,
        limit: Some(3),
        ..MessageQuery::default()
    };
    let first = server
        .client
        .list_messages("emails", &page(None))
        .await
        .unwrap();
    let second = server
        .client
        .list_messages("emails", &page(first.next_after))
        .await
        .unwrap();
    let read: Vec<_> = first
        .messages
        .iter()
        .chain(&second.messages)
        .map(|m| m.body.clone())
        .collect();
    assert_eq!(read, bodies);
    assert_eq!(second.next_after, None);
    assert_eq!(
        failure(server.client.list_messages("nope", &page(None)).await),
        (StatusCode::NOT_FOUND, "queue_not_found".into())
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_publish_step_fails_without_publishing_on_a_missing_queue_or_a_secret(pool: PgPool) {
    let steps = json!([{
        "id": "send",
        "action": "queue.publish",
        "params": { "queue": "{{ input.queue }}", "body": "{{ input.body }}" },
    }]);
    let (server, _workers) = publishing(pool, steps).await;

    let run = server
        .client
        .trigger_run("publisher", &json!({ "queue": "gone", "body": 1 }), None)
        .await
        .unwrap();
    let (status, timeline) = finished(&server, run.id).await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(timeline[0].error.as_ref().unwrap()["code"], "unknown_queue");

    let huge = "x".repeat(256 * 1024);
    let run = server
        .client
        .trigger_run(
            "publisher",
            &json!({ "queue": "emails", "body": huge }),
            None,
        )
        .await
        .unwrap();
    let (status, timeline) = finished(&server, run.id).await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(
        timeline[0].error.as_ref().unwrap()["code"],
        "invalid_params"
    );
    assert!(messages(&server, "emails").await.is_empty());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_publish_step_cannot_put_a_secret_in_a_message(pool: PgPool) {
    let steps = json!([{
        "id": "send",
        "action": "queue.publish",
        "params": { "queue": "emails", "body": "{{ secrets.API_TOKEN }}" },
    }]);
    let (server, _workers) = publishing(pool, steps).await;

    let run = server
        .client
        .trigger_run("publisher", &json!({}), None)
        .await
        .unwrap();
    let (status, timeline) = finished(&server, run.id).await;
    assert_eq!(status, RunStatus::Failed);
    let error = timeline[0].error.as_ref().unwrap();
    assert_eq!(error["code"], "invalid_params");
    assert!(error["message"].as_str().unwrap().contains("Secrets"));
    assert!(messages(&server, "emails").await.is_empty());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_dry_run_of_a_publish_step_publishes_nothing(pool: PgPool) {
    let server = TestServer::start(pool).await;
    server
        .client
        .put_queue("emails", &QueueSettings::default())
        .await
        .unwrap();
    let definition = publisher(json!([{
        "id": "send",
        "action": "queue.publish",
        "params": { "queue": "emails", "body": 1 },
    }]));

    let report = server
        .client
        .dry_run_definition(&definition, &json!({}), &json!({}))
        .await
        .unwrap();
    assert_eq!(report.status, RunStatus::Succeeded);
    assert_eq!(report.timeline[0].output.as_ref().unwrap()["published"], 1);
    assert!(messages(&server, "emails").await.is_empty());
}
