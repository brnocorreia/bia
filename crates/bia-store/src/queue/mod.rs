//! Queues and their Messages: publish with a delay, claim under a visibility timeout, ack,
//! retry with backoff, and dead-letter once retries run out (ADR-0015, ADR-0059).

mod model;
mod repository;
mod row;

pub use model::{
    Delivery, Message, MessageFilter, MessageId, MessageLease, Nacked, Queue, QueueSettings,
};
pub use repository::{PgQueueRepository, QueueRepository};
