use std::collections::HashMap;
use std::future::Future;

use bia_core::dsl::QueueName;
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;
use sqlx::{Acquire, PgConnection};

use super::row::{MessageRow, QueueRow, queue_name};
use super::{
    Delivery, Message, MessageCounts, MessageFilter, MessageId, MessageLease, Nacked, Queue,
    QueueSettings, Settlement,
};
use crate::StoreError;
use crate::job::Backoff;
use crate::run::RunId;

/// Queues and the lifecycle of their Messages (ADR-0059). Every method takes a connection, so
/// a publish can commit with the Checkpoint of the Step that publishes (ADR-0010), and takes
/// `now` from the caller's `Clock` (ADR-0022). A delivery's Lease is fenced by the Message's
/// delivery counter, as a Job's is by its attempts (ADR-0031): once the Message is delivered
/// again after its visibility timeout ran out, the earlier consumer can no longer ack, retry
/// or extend it.
pub trait QueueRepository {
    /// Creates the Queue, or replaces every setting of an existing one, keeping `created_at`.
    /// Fails with [`StoreError::DeadLetterQueueNotFound`] when `settings.dead_letter` names no
    /// Queue.
    fn put(
        &self,
        conn: &mut PgConnection,
        name: &QueueName,
        settings: &QueueSettings,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Queue, StoreError>> + Send;

    fn get(
        &self,
        conn: &mut PgConnection,
        name: &QueueName,
    ) -> impl Future<Output = Result<Option<Queue>, StoreError>> + Send;

    /// Every Queue, ordered by name.
    fn list(
        &self,
        conn: &mut PgConnection,
    ) -> impl Future<Output = Result<Vec<Queue>, StoreError>> + Send;

    /// Deletes the Queue with its Messages. Returns whether it existed; fails with
    /// [`StoreError::QueueInUse`] while another Queue names it as its Dead-Letter Queue.
    fn delete(
        &self,
        conn: &mut PgConnection,
        name: &QueueName,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Adds one Message per body, in order, claimable from `visible_at`. Fails with
    /// [`StoreError::QueueNotFound`] when the Queue does not exist.
    fn publish(
        &self,
        conn: &mut PgConnection,
        queue: &QueueName,
        bodies: &[Value],
        visible_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<MessageId>, StoreError>> + Send;

    /// Delivers up to `limit` claimable Messages of `queue` to `consumer`, oldest first, each
    /// hidden from other consumers for the Queue's visibility timeout. A Message whose last
    /// delivery timed out is delivered again, or dead-lettered when it has no retries left.
    /// Messages another consumer is claiming right now are skipped rather than waited for.
    fn claim(
        &self,
        conn: &mut PgConnection,
        queue: &QueueName,
        consumer: &str,
        limit: u32,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<Delivery>, StoreError>> + Send;

    /// Hides the Message for another visibility timeout from `now`.
    fn extend(
        &self,
        conn: &mut PgConnection,
        lease: &MessageLease,
        visibility_timeout: TimeDelta,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<MessageLease, StoreError>> + Send;

    /// Completes the delivery and deletes the Message.
    fn ack(
        &self,
        conn: &mut PgConnection,
        lease: &MessageLease,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Records that the delivery failed with `error`. The Message is delivered again after
    /// `backoff`, or dead-lettered once it used `max_retries` retries.
    fn nack(
        &self,
        conn: &mut PgConnection,
        lease: &MessageLease,
        error: &str,
        backoff: &Backoff,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Nacked, StoreError>> + Send;

    /// Hands claimed Messages to `run`, which holds them until [`settle`](Self::settle): no
    /// consumer receives them meanwhile, their visibility timeout no longer runs, and the
    /// Leases of their claim can no longer ack, retry or extend them. Call it in the
    /// transaction that claimed them and created the Run.
    fn hold(
        &self,
        conn: &mut PgConnection,
        messages: &[MessageId],
        run: RunId,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Applies `settlement` to every Message `run` holds, and returns how many there were.
    /// Call it in the transaction that finishes the Run.
    fn settle(
        &self,
        conn: &mut PgConnection,
        run: RunId,
        settlement: &Settlement,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    fn list_messages(
        &self,
        conn: &mut PgConnection,
        filter: &MessageFilter,
    ) -> impl Future<Output = Result<Vec<Message>, StoreError>> + Send;

    /// Message counts of `queue`, or of every Queue with `None`, as of `now`. A Queue holding
    /// no Messages has no entry.
    fn count_messages(
        &self,
        conn: &mut PgConnection,
        queue: Option<&QueueName>,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<HashMap<QueueName, MessageCounts>, StoreError>> + Send;

    /// Makes the dead letters among `ids` in `queue` deliverable again from `now` in the Queue
    /// they were dead-lettered from, with their delivery count reset, and returns their new
    /// ids. Skips ids that are not dead letters of `queue`, dead letters a consumer holds
    /// right now, and those whose source Queue was deleted.
    fn replay_dead_letters(
        &self,
        conn: &mut PgConnection,
        queue: &QueueName,
        ids: &[MessageId],
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<MessageId>, StoreError>> + Send;

    /// Deletes the dead letters among `ids` in `queue` and returns how many went.
    fn purge_dead_letters(
        &self,
        conn: &mut PgConnection,
        queue: &QueueName,
        ids: &[MessageId],
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Deletes up to `limit` dead letters dead-lettered before `before`, oldest first, and
    /// returns how many went.
    fn delete_dead_letters(
        &self,
        conn: &mut PgConnection,
        before: DateTime<Utc>,
        limit: u32,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgQueueRepository;

const FOREIGN_KEY_VIOLATION: &str = "23503";

fn violates_foreign_key(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(e) if e.code().as_deref() == Some(FOREIGN_KEY_VIOLATION))
}

fn to_i32(what: &'static str, value: u32) -> Result<i32, StoreError> {
    i32::try_from(value).map_err(|e| StoreError::invalid(what, e))
}

impl QueueRepository for PgQueueRepository {
    async fn put(
        &self,
        conn: &mut PgConnection,
        name: &QueueName,
        settings: &QueueSettings,
        now: DateTime<Utc>,
    ) -> Result<Queue, StoreError> {
        let row = sqlx::query_file_as!(
            QueueRow,
            "src/queue/queries/put.sql",
            name.as_str(),
            to_i32("queue max_retries", settings.max_retries)?,
            settings.visibility_timeout.num_milliseconds(),
            to_i32("queue batch_size", settings.batch_size)?,
            settings.dead_letter.as_ref().map(QueueName::as_str),
            now,
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(|error| match &settings.dead_letter {
            Some(dead_letter) if violates_foreign_key(&error) => {
                StoreError::DeadLetterQueueNotFound(dead_letter.clone())
            }
            _ => error.into(),
        })?;
        row.try_into()
    }

    async fn get(
        &self,
        conn: &mut PgConnection,
        name: &QueueName,
    ) -> Result<Option<Queue>, StoreError> {
        sqlx::query_file_as!(QueueRow, "src/queue/queries/get.sql", name.as_str())
            .fetch_optional(&mut *conn)
            .await?
            .map(Queue::try_from)
            .transpose()
    }

    async fn list(&self, conn: &mut PgConnection) -> Result<Vec<Queue>, StoreError> {
        sqlx::query_file_as!(QueueRow, "src/queue/queries/list.sql")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(Queue::try_from)
            .collect()
    }

    async fn delete(&self, conn: &mut PgConnection, name: &QueueName) -> Result<bool, StoreError> {
        let done = sqlx::query_file!("src/queue/queries/delete.sql", name.as_str())
            .execute(&mut *conn)
            .await
            .map_err(|error| {
                if violates_foreign_key(&error) {
                    StoreError::QueueInUse(name.clone())
                } else {
                    error.into()
                }
            })?;
        Ok(done.rows_affected() > 0)
    }

    async fn publish(
        &self,
        conn: &mut PgConnection,
        queue: &QueueName,
        bodies: &[Value],
        visible_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Vec<MessageId>, StoreError> {
        let ids = sqlx::query_file_scalar!(
            "src/queue/queries/publish.sql",
            queue.as_str(),
            bodies,
            now,
            visible_at,
        )
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| {
            if violates_foreign_key(&error) {
                StoreError::QueueNotFound(queue.clone())
            } else {
                error.into()
            }
        })?;
        Ok(ids.into_iter().map(MessageId::new).collect())
    }

    async fn claim(
        &self,
        conn: &mut PgConnection,
        queue: &QueueName,
        consumer: &str,
        limit: u32,
        now: DateTime<Utc>,
    ) -> Result<Vec<Delivery>, StoreError> {
        let mut tx = conn.begin().await?;
        let settings = self
            .get(&mut tx, queue)
            .await?
            .ok_or_else(|| StoreError::QueueNotFound(queue.clone()))?
            .settings;
        let max_retries = to_i32("queue max_retries", settings.max_retries)?;

        let expired = sqlx::query_file_scalar!(
            "src/queue/queries/expired.sql",
            queue.as_str(),
            now,
            max_retries,
        )
        .fetch_all(&mut *tx)
        .await?;
        if !expired.is_empty() {
            dead_letter(
                &mut tx,
                &expired,
                now,
                Some("the visibility timeout ran out before the Message was acked"),
            )
            .await?;
        }

        let until = now + settings.visibility_timeout;
        let mut delivered = sqlx::query_file_as!(
            MessageRow,
            "src/queue/queries/claim.sql",
            queue.as_str(),
            consumer,
            now,
            i64::from(limit),
            until,
            max_retries,
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| {
            let message = Message::try_from(row)?;
            let lease = MessageLease::new(message.id, message.deliveries, until);
            Ok(Delivery { message, lease })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
        tx.commit().await?;

        // `RETURNING` has no order of its own.
        delivered.sort_by_key(|delivery| delivery.message.id);
        Ok(delivered)
    }

    async fn extend(
        &self,
        conn: &mut PgConnection,
        lease: &MessageLease,
        visibility_timeout: TimeDelta,
        now: DateTime<Utc>,
    ) -> Result<MessageLease, StoreError> {
        let until = now + visibility_timeout;
        let extended = sqlx::query_file!(
            "src/queue/queries/extend.sql",
            lease.message_id().get(),
            lease.delivery(),
            until,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if extended == 0 {
            return Err(StoreError::MessageLeaseLost(lease.message_id()));
        }
        Ok(MessageLease::new(
            lease.message_id(),
            lease.delivery(),
            until,
        ))
    }

    async fn ack(&self, conn: &mut PgConnection, lease: &MessageLease) -> Result<(), StoreError> {
        let deleted = sqlx::query_file!(
            "src/queue/queries/ack.sql",
            lease.message_id().get(),
            lease.delivery(),
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if deleted == 0 {
            return Err(StoreError::MessageLeaseLost(lease.message_id()));
        }
        Ok(())
    }

    async fn nack(
        &self,
        conn: &mut PgConnection,
        lease: &MessageLease,
        error: &str,
        backoff: &Backoff,
        now: DateTime<Utc>,
    ) -> Result<Nacked, StoreError> {
        let mut tx = conn.begin().await?;
        let id = lease.message_id().get();
        let held = sqlx::query_file!("src/queue/queries/lock_delivery.sql", id, lease.delivery())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StoreError::MessageLeaseLost(lease.message_id()))?;
        let nacked = if held.deliveries > held.max_retries {
            dead_letter(&mut tx, &[id], now, Some(error)).await?;
            Nacked::DeadLettered
        } else {
            let visible_at = now + backoff.delay(held.deliveries);
            sqlx::query_file!("src/queue/queries/retry.sql", id, visible_at, error)
                .execute(&mut *tx)
                .await?;
            Nacked::Retrying { visible_at }
        };
        tx.commit().await?;
        Ok(nacked)
    }

    async fn hold(
        &self,
        conn: &mut PgConnection,
        messages: &[MessageId],
        run: RunId,
    ) -> Result<(), StoreError> {
        let ids: Vec<i64> = messages.iter().map(|id| id.get()).collect();
        sqlx::query_file!("src/queue/queries/hold.sql", &ids, run.as_uuid())
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    async fn settle(
        &self,
        conn: &mut PgConnection,
        run: RunId,
        settlement: &Settlement,
        now: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        let (error, backoff) = match settlement {
            Settlement::Ack => {
                let acked = sqlx::query_file!("src/queue/queries/ack_held.sql", run.as_uuid())
                    .execute(&mut *conn)
                    .await?
                    .rows_affected();
                return Ok(acked);
            }
            Settlement::Retry { error, backoff } => (error, Some(backoff)),
            Settlement::DeadLetter { error } => (error, None),
        };
        let mut tx = conn.begin().await?;
        let held = sqlx::query_file!("src/queue/queries/lock_held.sql", run.as_uuid())
            .fetch_all(&mut *tx)
            .await?;
        let mut exhausted = Vec::new();
        let (mut retried, mut visible_at) = (Vec::new(), Vec::new());
        for message in &held {
            match backoff {
                Some(backoff) if message.deliveries <= message.max_retries => {
                    retried.push(message.id);
                    visible_at.push(now + backoff.delay(message.deliveries));
                }
                _ => exhausted.push(message.id),
            }
        }
        if !exhausted.is_empty() {
            dead_letter(&mut tx, &exhausted, now, Some(error)).await?;
        }
        if !retried.is_empty() {
            sqlx::query_file!(
                "src/queue/queries/retry_held.sql",
                &retried,
                &visible_at,
                error,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(held.len() as u64)
    }

    async fn list_messages(
        &self,
        conn: &mut PgConnection,
        filter: &MessageFilter,
    ) -> Result<Vec<Message>, StoreError> {
        sqlx::query_file_as!(
            MessageRow,
            "src/queue/queries/list_messages.sql",
            filter.queue.as_str(),
            filter.dead_letters,
            filter.after.map(MessageId::get),
            i64::from(filter.limit),
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(Message::try_from)
        .collect()
    }

    async fn count_messages(
        &self,
        conn: &mut PgConnection,
        queue: Option<&QueueName>,
        now: DateTime<Utc>,
    ) -> Result<HashMap<QueueName, MessageCounts>, StoreError> {
        let count =
            |what, value: i64| u64::try_from(value).map_err(|e| StoreError::invalid(what, e));
        sqlx::query_file!(
            "src/queue/queries/count.sql",
            now,
            queue.map(QueueName::as_str),
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|row| {
            let name = queue_name(row.queue)?;
            let counts = MessageCounts {
                ready: count("ready count", row.ready)?,
                in_flight: count("in-flight count", row.in_flight)?,
                delayed: count("delayed count", row.delayed)?,
                dead_lettered: count("dead-lettered count", row.dead_lettered)?,
            };
            Ok((name, counts))
        })
        .collect()
    }

    async fn replay_dead_letters(
        &self,
        conn: &mut PgConnection,
        queue: &QueueName,
        ids: &[MessageId],
        now: DateTime<Utc>,
    ) -> Result<Vec<MessageId>, StoreError> {
        let ids: Vec<i64> = ids.iter().map(|id| id.get()).collect();
        let replayed = sqlx::query_file_scalar!(
            "src/queue/queries/replay_dead_letters.sql",
            queue.as_str(),
            &ids,
            now,
        )
        .fetch_all(&mut *conn)
        .await?;
        Ok(replayed.into_iter().map(MessageId::new).collect())
    }

    async fn purge_dead_letters(
        &self,
        conn: &mut PgConnection,
        queue: &QueueName,
        ids: &[MessageId],
    ) -> Result<u64, StoreError> {
        let ids: Vec<i64> = ids.iter().map(|id| id.get()).collect();
        let purged = sqlx::query_file!(
            "src/queue/queries/purge_dead_letters.sql",
            queue.as_str(),
            &ids,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();
        Ok(purged)
    }

    async fn delete_dead_letters(
        &self,
        conn: &mut PgConnection,
        before: DateTime<Utc>,
        limit: u32,
    ) -> Result<u64, StoreError> {
        let deleted = sqlx::query_file!(
            "src/queue/queries/delete_dead_letters.sql",
            before,
            i64::from(limit),
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();
        Ok(deleted)
    }
}

async fn dead_letter(
    conn: &mut PgConnection,
    ids: &[i64],
    now: DateTime<Utc>,
    error: Option<&str>,
) -> Result<(), StoreError> {
    sqlx::query_file!("src/queue/queries/dead_letter.sql", ids, now, error)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
