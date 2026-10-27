//! Queues and their Messages: publish with a delay, claim under a visibility timeout, ack,
//! retry with backoff, and dead-letter once retries run out (ADR-0015, ADR-0059). Messages a
//! queue Trigger delivers are held by their Run until it finishes, which settles them
//! (ADR-0061).

mod model;
mod repository;
mod row;

pub use model::{
    Delivery, Message, MessageCounts, MessageFilter, MessageId, MessageLease, Nacked, Queue,
    QueueSettings, Settlement,
};
pub use repository::{PgQueueRepository, QueueRepository};
