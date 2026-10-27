//! Queue Triggers: each dispatch claims a batch of Messages and starts a Run of the Active
//! Version that holds them until it finishes, all in one transaction (ADR-0061).

use bia_core::dsl::MAX_BATCH_SIZE;
use bia_store::StoreError;
use bia_store::queue::{Delivery, PgQueueRepository, QueueRepository};
use bia_store::run::RunId;
use bia_store::schedule::{PgScheduleRepository, ReadyConsumer, ScheduleRepository};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use tracing::Instrument;

use crate::{EngineError, start_run};

/// The most Runs one poll starts from queue Triggers, so a backlog cannot hold off leadership
/// checks.
const DISPATCHES_PER_POLL: usize = 1_000;

/// Starts Runs from every queue Trigger whose Queue has Messages claimable at `now`. Triggers
/// take one batch each in turn, so one with a backlog does not starve another on the same
/// Queue. A Trigger that cannot dispatch is logged and skipped until the next poll.
pub(super) async fn dispatch(pool: &PgPool, now: DateTime<Utc>) -> Result<Vec<RunId>, EngineError> {
    let mut conn = pool.acquire().await?;
    let mut ready = PgScheduleRepository.ready_consumers(&mut conn, now).await?;
    drop(conn);

    let mut started = Vec::new();
    while !ready.is_empty() {
        let mut more = Vec::new();
        for consumer in ready {
            if started.len() >= DISPATCHES_PER_POLL {
                return Ok(started);
            }
            let span = tracing::info_span!(
                "queue_trigger",
                workflow_id = %consumer.consumer.workflow_id,
                trigger_id = %consumer.consumer.trigger_id,
            );
            match dispatch_batch(pool, &consumer, now).instrument(span).await {
                Ok(Some(run)) => {
                    started.push(run);
                    more.push(consumer);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::error!(
                        workflow_id = %consumer.consumer.workflow_id,
                        trigger_id = %consumer.consumer.trigger_id,
                        %error,
                        "cannot dispatch messages; retrying on the next poll"
                    );
                }
            }
        }
        ready = more;
    }
    Ok(started)
}

/// Claims up to one batch and starts its Run. `None` when there was nothing to claim, or the
/// Trigger's Version is no longer the Active Version.
async fn dispatch_batch(
    pool: &PgPool,
    ready: &ReadyConsumer,
    now: DateTime<Utc>,
) -> Result<Option<RunId>, EngineError> {
    let consumer = &ready.consumer;
    let mut tx = pool.begin().await?;
    // Held until commit, so a Version deactivated meanwhile cannot start a Run afterwards.
    if !PgScheduleRepository
        .lock_active(&mut tx, &consumer.workflow_id, consumer.version)
        .await?
    {
        return Ok(None);
    }
    let name = format!("{}/{}", consumer.workflow_id, consumer.trigger_id);
    // Validation caps `batch_size`, but a Version published before it did may ask for more.
    let limit = ready.batch_size.min(MAX_BATCH_SIZE);
    let deliveries = match PgQueueRepository
        .claim(&mut tx, &consumer.queue, &name, limit, now)
        .await
    {
        Ok(deliveries) => deliveries,
        Err(StoreError::QueueNotFound(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if deliveries.is_empty() {
        // The claim may still have dead-lettered Messages whose last delivery timed out.
        tx.commit().await?;
        return Ok(None);
    }

    let input = json!({
        "messages": deliveries
            .iter()
            .map(|delivery| delivery.message.body.clone())
            .collect::<Vec<_>>(),
    });
    let trigger = trigger_metadata(ready, &deliveries);
    let run = start_run(
        &mut tx,
        &consumer.workflow_id,
        consumer.version,
        input,
        trigger,
        now,
    )
    .await?;
    let ids: Vec<_> = deliveries
        .iter()
        .map(|delivery| delivery.message.id)
        .collect();
    PgQueueRepository.hold(&mut tx, &ids, run.id).await?;
    tx.commit().await?;
    tracing::info!(run_id = %run.id, messages = ids.len(), "messages dispatched");
    Ok(Some(run.id))
}

/// The `trigger` of a Run a queue Trigger starts. `messages` follows the order of
/// `input.messages`.
fn trigger_metadata(ready: &ReadyConsumer, deliveries: &[Delivery]) -> Value {
    let messages: Vec<Value> = deliveries
        .iter()
        .map(|Delivery { message, .. }| {
            json!({
                "id": message.id.get(),
                "deliveries": message.deliveries,
                "published_at": message
                    .published_at
                    .to_rfc3339_opts(SecondsFormat::AutoSi, true),
            })
        })
        .collect();
    json!({
        "type": "queue",
        "id": ready.consumer.trigger_id.as_str(),
        "queue": ready.consumer.queue.as_str(),
        "messages": messages,
    })
}
