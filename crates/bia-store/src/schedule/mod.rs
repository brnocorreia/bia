//! The scheduler's materialised state for the Triggers of Active Versions: a Schedule per cron
//! Trigger and a [`QueueConsumer`] per queue Trigger, rebuilt together whenever the Active
//! Version changes, and the advisory lock that elects one scheduler to fire them (ADR-0039,
//! ADR-0061).

mod leader;
mod model;
mod repository;
mod row;

pub use leader::LeaderLock;
pub use model::{DueSchedule, QueueConsumer, ReadyConsumer, Schedule, SyncState};
pub use repository::{PgScheduleRepository, ScheduleRepository};
