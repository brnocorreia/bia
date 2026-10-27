use bia_core::dsl::QueueName;
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

use super::{Message, MessageId, Queue, QueueSettings};
use crate::StoreError;

#[derive(Debug)]
pub(crate) struct QueueRow {
    pub(crate) name: String,
    pub(crate) max_retries: i32,
    pub(crate) visibility_timeout_ms: i64,
    pub(crate) batch_size: i32,
    pub(crate) dead_letter: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

impl TryFrom<QueueRow> for Queue {
    type Error = StoreError;

    fn try_from(row: QueueRow) -> Result<Self, StoreError> {
        let count =
            |what, value: i32| u32::try_from(value).map_err(|e| StoreError::invalid(what, e));
        Ok(Self {
            name: queue_name(row.name)?,
            settings: QueueSettings {
                max_retries: count("queue max_retries", row.max_retries)?,
                visibility_timeout: TimeDelta::milliseconds(row.visibility_timeout_ms),
                batch_size: count("queue batch_size", row.batch_size)?,
                dead_letter: row.dead_letter.map(queue_name).transpose()?,
            },
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(Debug)]
pub(crate) struct MessageRow {
    pub(crate) id: i64,
    pub(crate) queue: String,
    pub(crate) body: Value,
    pub(crate) published_at: DateTime<Utc>,
    pub(crate) visible_at: Option<DateTime<Utc>>,
    pub(crate) deliveries: i32,
    pub(crate) claimed_by: Option<String>,
    pub(crate) last_error: Option<String>,
    pub(crate) dead_lettered_at: Option<DateTime<Utc>>,
    pub(crate) dead_lettered_from: Option<String>,
}

impl TryFrom<MessageRow> for Message {
    type Error = StoreError;

    fn try_from(row: MessageRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: MessageId::new(row.id),
            queue: queue_name(row.queue)?,
            body: row.body,
            published_at: row.published_at,
            visible_at: row.visible_at,
            deliveries: row.deliveries,
            claimed_by: row.claimed_by,
            last_error: row.last_error,
            dead_lettered_at: row.dead_lettered_at,
            dead_lettered_from: row.dead_lettered_from.map(queue_name).transpose()?,
        })
    }
}

pub(super) fn queue_name(name: String) -> Result<QueueName, StoreError> {
    QueueName::new(name).map_err(|e| StoreError::invalid("queue name", e))
}
