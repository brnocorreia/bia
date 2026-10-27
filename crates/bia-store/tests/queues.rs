//! Queues and the lifecycle of their Messages, with time driven by the manual Clock.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use bia_core::clock::{Clock, ManualClock};
use bia_core::dsl::QueueName;
use bia_store::StoreError;
use bia_store::job::Backoff;
use bia_store::queue::{
    Delivery, MessageFilter, MessageId, Nacked, PgQueueRepository, QueueRepository, QueueSettings,
};
use chrono::TimeDelta;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};

mod common;

use common::t0;

fn name(name: &str) -> QueueName {
    QueueName::new(name).unwrap()
}

/// Creates `emails`, retried `max_retries` times under a 30s visibility timeout, dead-lettering
/// into `emails-dlq`.
async fn emails(conn: &mut PgConnection, max_retries: u32) {
    let queues = PgQueueRepository;
    queues
        .put(conn, &name("emails-dlq"), &QueueSettings::default(), t0())
        .await
        .unwrap();
    let settings = QueueSettings {
        max_retries,
        visibility_timeout: TimeDelta::seconds(30),
        dead_letter: Some(name("emails-dlq")),
        ..QueueSettings::default()
    };
    queues
        .put(conn, &name("emails"), &settings, t0())
        .await
        .unwrap();
}

async fn publish(conn: &mut PgConnection, queue: &str, bodies: &[Value], clock: &ManualClock) {
    PgQueueRepository
        .publish(conn, &name(queue), bodies, clock.now(), clock.now())
        .await
        .unwrap();
}

async fn claim(conn: &mut PgConnection, queue: &str, clock: &ManualClock) -> Vec<Delivery> {
    PgQueueRepository
        .claim(conn, &name(queue), "consumer", 10, clock.now())
        .await
        .unwrap()
}

async fn messages_in(conn: &mut PgConnection, queue: &str) -> Vec<bia_store::queue::Message> {
    let filter = MessageFilter {
        queue: name(queue),
        dead_letters: None,
        after: None,
        limit: 100,
    };
    PgQueueRepository
        .list_messages(conn, &filter)
        .await
        .unwrap()
}

#[sqlx::test]
async fn many_concurrent_consumers_never_receive_the_same_message(pool: PgPool) {
    const MESSAGES: usize = 300;
    const CONSUMERS: usize = 16;

    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    emails(&mut conn, 3).await;
    let bodies: Vec<Value> = (0..MESSAGES).map(|i| json!({ "n": i })).collect();
    let mut published = PgQueueRepository
        .publish(
            &mut conn,
            &name("emails"),
            &bodies,
            clock.now(),
            clock.now(),
        )
        .await
        .unwrap();
    drop(conn);

    let received: Arc<Mutex<Vec<(MessageId, i32)>>> = Arc::default();
    let mut consumers = Vec::new();
    for c in 0..CONSUMERS {
        let pool = pool.clone();
        let clock = clock.clone();
        let received = Arc::clone(&received);
        consumers.push(tokio::spawn(async move {
            let consumer = format!("consumer-{c}");
            loop {
                let mut conn = pool.acquire().await.unwrap();
                let batch = PgQueueRepository
                    .claim(&mut conn, &name("emails"), &consumer, 7, clock.now())
                    .await
                    .unwrap();
                drop(conn);
                if batch.is_empty() {
                    break;
                }
                for delivery in batch {
                    tokio::task::yield_now().await;
                    let mut conn = pool.acquire().await.unwrap();
                    PgQueueRepository
                        .ack(&mut conn, &delivery.lease)
                        .await
                        .unwrap();
                    received
                        .lock()
                        .unwrap()
                        .push((delivery.message.id, delivery.lease.delivery()));
                }
            }
        }));
    }
    for consumer in consumers {
        consumer.await.unwrap();
    }

    let received = std::mem::take(&mut *received.lock().unwrap());
    let mut times: BTreeMap<MessageId, usize> = BTreeMap::new();
    for (id, delivery) in &received {
        assert_eq!(*delivery, 1, "message {id} was delivered twice");
        *times.entry(*id).or_default() += 1;
    }
    published.sort();
    assert_eq!(times.keys().copied().collect::<Vec<_>>(), published);
    assert!(times.values().all(|times| *times == 1));
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[sqlx::test]
async fn a_delayed_message_is_claimable_once_its_delay_elapses(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    emails(&mut conn, 3).await;
    PgQueueRepository
        .publish(
            &mut conn,
            &name("emails"),
            &[json!("later")],
            clock.now() + TimeDelta::minutes(5),
            clock.now(),
        )
        .await
        .unwrap();

    clock.advance(TimeDelta::minutes(5) - TimeDelta::milliseconds(1));
    assert!(claim(&mut conn, "emails", &clock).await.is_empty());
    clock.advance(TimeDelta::milliseconds(1));
    let delivered = claim(&mut conn, "emails", &clock).await;
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].message.body, json!("later"));
    assert_eq!(delivered[0].message.published_at, t0());
}

#[sqlx::test]
async fn an_unacked_message_reappears_after_the_visibility_timeout_until_it_is_dead_lettered(
    pool: PgPool,
) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    emails(&mut conn, 2).await;
    publish(
        &mut conn,
        "emails",
        &[json!({ "to": "a@example.com" })],
        &clock,
    )
    .await;

    let first = claim(&mut conn, "emails", &clock).await;
    assert_eq!(first.len(), 1);
    clock.advance(TimeDelta::seconds(29));
    assert!(claim(&mut conn, "emails", &clock).await.is_empty());

    // The first delivery and its two retries all time out.
    let mut leases = vec![first[0].lease.clone()];
    for delivery in 2..=3 {
        clock.advance(TimeDelta::seconds(1));
        let again = claim(&mut conn, "emails", &clock).await;
        assert_eq!(again.len(), 1, "delivery {delivery}");
        assert_eq!(again[0].lease.delivery(), delivery);
        leases.push(again[0].lease.clone());
        clock.advance(TimeDelta::seconds(29));
    }
    clock.advance(TimeDelta::seconds(1));
    assert!(claim(&mut conn, "emails", &clock).await.is_empty());

    assert!(messages_in(&mut conn, "emails").await.is_empty());
    let dead = messages_in(&mut conn, "emails-dlq").await;
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].body, json!({ "to": "a@example.com" }));
    assert_eq!(dead[0].deliveries, 0);
    assert_eq!(dead[0].dead_lettered_at, Some(clock.now()));
    assert_eq!(dead[0].dead_lettered_from, Some(name("emails")));
    assert_eq!(dead[0].published_at, t0());

    // No earlier delivery can ack the dead letter, which the Dead-Letter Queue delivers anew.
    for lease in &leases {
        assert!(matches!(
            PgQueueRepository.ack(&mut conn, lease).await,
            Err(StoreError::MessageLeaseLost(_))
        ));
    }
    let redelivered = claim(&mut conn, "emails-dlq", &clock).await;
    assert_eq!(redelivered.len(), 1);
    assert_eq!(redelivered[0].lease.delivery(), 1);
}

#[sqlx::test]
async fn a_late_ack_after_redelivery_is_refused(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    emails(&mut conn, 3).await;
    publish(&mut conn, "emails", &[json!(1)], &clock).await;

    let first = claim(&mut conn, "emails", &clock).await.remove(0);
    clock.advance(TimeDelta::seconds(30));
    let second = claim(&mut conn, "emails", &clock).await.remove(0);

    assert!(matches!(
        PgQueueRepository.ack(&mut conn, &first.lease).await,
        Err(StoreError::MessageLeaseLost(_))
    ));
    PgQueueRepository
        .ack(&mut conn, &second.lease)
        .await
        .unwrap();
    assert!(messages_in(&mut conn, "emails").await.is_empty());
}

#[sqlx::test]
async fn an_extended_delivery_stays_hidden(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    emails(&mut conn, 3).await;
    publish(&mut conn, "emails", &[json!(1)], &clock).await;

    let delivery = claim(&mut conn, "emails", &clock).await.remove(0);
    clock.advance(TimeDelta::seconds(20));
    let extended = PgQueueRepository
        .extend(
            &mut conn,
            &delivery.lease,
            TimeDelta::seconds(30),
            clock.now(),
        )
        .await
        .unwrap();
    assert_eq!(extended.until(), clock.now() + TimeDelta::seconds(30));
    clock.advance(TimeDelta::seconds(20));
    assert!(claim(&mut conn, "emails", &clock).await.is_empty());
    PgQueueRepository.ack(&mut conn, &extended).await.unwrap();
}

#[sqlx::test]
async fn a_nacked_message_retries_with_backoff_then_is_dead_lettered(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    emails(&mut conn, 1).await;
    publish(&mut conn, "emails", &[json!(1)], &clock).await;
    let backoff = Backoff {
        initial: TimeDelta::seconds(10),
        max: TimeDelta::minutes(1),
    };

    let first = claim(&mut conn, "emails", &clock).await.remove(0);
    let nacked = PgQueueRepository
        .nack(&mut conn, &first.lease, "smtp down", &backoff, clock.now())
        .await
        .unwrap();
    assert_eq!(
        nacked,
        Nacked::Retrying {
            visible_at: t0() + TimeDelta::seconds(10)
        }
    );
    clock.advance(TimeDelta::seconds(9));
    assert!(claim(&mut conn, "emails", &clock).await.is_empty());
    clock.advance(TimeDelta::seconds(1));
    let second = claim(&mut conn, "emails", &clock).await.remove(0);
    assert_eq!(second.message.last_error.as_deref(), Some("smtp down"));

    let nacked = PgQueueRepository
        .nack(
            &mut conn,
            &second.lease,
            "still down",
            &backoff,
            clock.now(),
        )
        .await
        .unwrap();
    assert_eq!(nacked, Nacked::DeadLettered);
    let dead = messages_in(&mut conn, "emails-dlq").await;
    assert_eq!(dead[0].last_error.as_deref(), Some("still down"));
    assert!(matches!(
        PgQueueRepository
            .nack(&mut conn, &second.lease, "again", &backoff, clock.now())
            .await,
        Err(StoreError::MessageLeaseLost(_))
    ));
}

#[sqlx::test]
async fn without_a_dead_letter_queue_an_exhausted_message_stays_and_is_never_delivered(
    pool: PgPool,
) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    let settings = QueueSettings {
        max_retries: 0,
        ..QueueSettings::default()
    };
    PgQueueRepository
        .put(&mut conn, &name("audit"), &settings, t0())
        .await
        .unwrap();
    publish(&mut conn, "audit", &[json!(1)], &clock).await;

    claim(&mut conn, "audit", &clock).await;
    clock.advance(TimeDelta::hours(1));
    assert!(claim(&mut conn, "audit", &clock).await.is_empty());

    let kept = messages_in(&mut conn, "audit").await;
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].visible_at, None);
    assert_eq!(kept[0].dead_lettered_from, Some(name("audit")));
    let filter = |dead_letters| MessageFilter {
        queue: name("audit"),
        dead_letters: Some(dead_letters),
        after: None,
        limit: 10,
    };
    let queues = PgQueueRepository;
    assert_eq!(
        queues
            .list_messages(&mut conn, &filter(true))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        queues
            .list_messages(&mut conn, &filter(false))
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test]
async fn dead_letters_older_than_the_cutoff_are_deleted_in_batches(pool: PgPool) {
    let clock = ManualClock::new(t0());
    let mut conn = pool.acquire().await.unwrap();
    emails(&mut conn, 0).await;
    publish(&mut conn, "emails", &[json!(1), json!(2), json!(3)], &clock).await;
    publish(
        &mut conn,
        "emails-dlq",
        &[json!("not a dead letter")],
        &clock,
    )
    .await;
    claim(&mut conn, "emails", &clock).await;
    clock.advance(TimeDelta::seconds(30));
    claim(&mut conn, "emails", &clock).await;

    let queues = PgQueueRepository;
    let at = clock.now();
    assert_eq!(
        queues.delete_dead_letters(&mut conn, at, 10).await.unwrap(),
        0
    );
    let after = at + TimeDelta::milliseconds(1);
    assert_eq!(
        queues
            .delete_dead_letters(&mut conn, after, 2)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        queues
            .delete_dead_letters(&mut conn, after, 2)
            .await
            .unwrap(),
        1
    );
    let left = messages_in(&mut conn, "emails-dlq").await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].body, json!("not a dead letter"));
}

#[sqlx::test]
async fn queues_are_replaced_by_put_and_guard_their_dead_letter_queues(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let queues = PgQueueRepository;
    let orphan = QueueSettings {
        dead_letter: Some(name("missing")),
        ..QueueSettings::default()
    };
    assert!(matches!(
        queues.put(&mut conn, &name("emails"), &orphan, t0()).await,
        Err(StoreError::DeadLetterQueueNotFound(dlq)) if dlq == name("missing")
    ));

    emails(&mut conn, 5).await;
    let later = t0() + TimeDelta::minutes(1);
    let replaced = queues
        .put(&mut conn, &name("emails"), &QueueSettings::default(), later)
        .await
        .unwrap();
    assert_eq!(replaced.settings, QueueSettings::default());
    assert_eq!((replaced.created_at, replaced.updated_at), (t0(), later));

    queues
        .put(
            &mut conn,
            &name("emails"),
            &QueueSettings {
                dead_letter: Some(name("emails-dlq")),
                ..QueueSettings::default()
            },
            later,
        )
        .await
        .unwrap();
    assert!(matches!(
        queues.delete(&mut conn, &name("emails-dlq")).await,
        Err(StoreError::QueueInUse(_))
    ));
    let clock = ManualClock::new(t0());
    publish(&mut conn, "emails", &[json!(1)], &clock).await;
    assert!(queues.delete(&mut conn, &name("emails")).await.unwrap());
    assert!(!queues.delete(&mut conn, &name("emails")).await.unwrap());
    assert!(queues.delete(&mut conn, &name("emails-dlq")).await.unwrap());
    assert!(matches!(
        queues
            .publish(&mut conn, &name("emails"), &[json!(1)], t0(), t0())
            .await,
        Err(StoreError::QueueNotFound(_))
    ));
}
