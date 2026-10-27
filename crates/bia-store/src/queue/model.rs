use std::fmt;

use bia_core::dsl::{
    DEFAULT_BATCH_SIZE, DEFAULT_MAX_RETRIES, QueueName, default_visibility_timeout,
};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

/// What a Queue is configured with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSettings {
    /// Deliveries a Message gets after its first before it is dead-lettered.
    pub max_retries: u32,
    pub visibility_timeout: TimeDelta,
    pub batch_size: u32,
    pub dead_letter: Option<QueueName>,
}

impl Default for QueueSettings {
    fn default() -> Self {
        let visibility = default_visibility_timeout().as_millis();
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            visibility_timeout: TimeDelta::milliseconds(i64::try_from(visibility).unwrap_or(0)),
            batch_size: DEFAULT_BATCH_SIZE,
            dead_letter: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queue {
    pub name: QueueName,
    pub settings: QueueSettings,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageId(i64);

impl MessageId {
    pub fn new(id: i64) -> Self {
        Self(id)
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub id: MessageId,
    pub queue: QueueName,
    pub body: Value,
    pub published_at: DateTime<Utc>,
    /// When the Message can be claimed next. `None` for a dead letter kept in a Queue without
    /// a Dead-Letter Queue, which is never delivered again.
    pub visible_at: Option<DateTime<Utc>>,
    /// Deliveries in this Queue so far; a dead letter starts again from 0 in the Dead-Letter
    /// Queue.
    pub deliveries: i32,
    /// The consumer of the latest delivery, until it is acked, retried or dead-lettered.
    pub claimed_by: Option<String>,
    pub last_error: Option<String>,
    pub dead_lettered_at: Option<DateTime<Utc>>,
    /// The Queue the Message exhausted its retries in.
    pub dead_lettered_from: Option<QueueName>,
}

/// A time-bounded claim on one delivery of a Message: until the visibility timeout ends, no
/// other consumer receives the Message. Only the delivery that produced it can ack, retry or
/// extend it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageLease {
    message_id: MessageId,
    delivery: i32,
    until: DateTime<Utc>,
}

impl MessageLease {
    pub(crate) fn new(message_id: MessageId, delivery: i32, until: DateTime<Utc>) -> Self {
        Self {
            message_id,
            delivery,
            until,
        }
    }

    pub fn message_id(&self) -> MessageId {
        self.message_id
    }

    /// Which delivery of the Message this Lease belongs to, starting at 1.
    pub fn delivery(&self) -> i32 {
        self.delivery
    }

    pub fn until(&self) -> DateTime<Utc> {
        self.until
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    pub message: Message,
    pub lease: MessageLease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nacked {
    /// The Message is delivered again from `visible_at`.
    Retrying { visible_at: DateTime<Utc> },
    /// The Message used its last retry and was dead-lettered.
    DeadLettered,
}

/// How many Messages a Queue holds, by state. The counts do not overlap: a dead letter counts
/// only as `dead_lettered`, even in a Dead-Letter Queue whose consumers still receive it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MessageCounts {
    /// Claimable now, including Messages whose delivery's visibility timeout ran out.
    pub ready: u64,
    /// Held by a consumer within its visibility timeout.
    pub in_flight: u64,
    /// Waiting out a publish delay or a retry backoff.
    pub delayed: u64,
    pub dead_lettered: u64,
}

/// Which of a Queue's Messages to list, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFilter {
    pub queue: QueueName,
    /// Only dead letters with `Some(true)`, only the others with `Some(false)`.
    pub dead_letters: Option<bool>,
    pub after: Option<MessageId>,
    pub limit: u32,
}
