use bia_core::dsl::{CronTrigger, QueueName, TriggerId, WorkflowId};
use chrono::{DateTime, Utc};

use crate::run::RunId;
use crate::version::VersionNumber;

/// The scheduler's materialised state for one cron Trigger of an Active Version.
#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub workflow_id: WorkflowId,
    pub trigger_id: TriggerId,
    /// The Version the Trigger comes from, which Runs it starts are pinned to.
    pub version: VersionNumber,
    /// The Trigger's settings, with `overlap` and `catch_up` always set.
    pub trigger: CronTrigger,
    /// The next occurrence of the expression that has not fired yet.
    pub next_at: DateTime<Utc>,
    /// When `next_at` fires: `next_at` plus its jitter.
    pub due_at: DateTime<Utc>,
    /// Occurrences held back by `overlap: queue` until the previous Run finishes, oldest
    /// first.
    pub queued: Vec<DateTime<Utc>>,
    /// The Run this Schedule started last; `None` once retention deleted it.
    pub last_run: Option<RunId>,
}

/// A Schedule with something to fire, locked by the transaction that claimed it.
#[derive(Debug, Clone, PartialEq)]
pub struct DueSchedule {
    pub schedule: Schedule,
    /// Whether `last_run` has not finished yet.
    pub previous_active: bool,
}

/// A Workflow's Active Version and the Version its Schedules were built from. They differ
/// until the scheduler rebuilds the Schedules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncState {
    pub active: Option<VersionNumber>,
    pub scheduled: Option<VersionNumber>,
}

/// A queue Trigger of an Active Version, which the scheduler starts Runs from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueConsumer {
    pub workflow_id: WorkflowId,
    pub trigger_id: TriggerId,
    /// The Version the Trigger comes from, which Runs it starts are pinned to.
    pub version: VersionNumber,
    pub queue: QueueName,
    /// Messages per Run; `None` takes the Queue's `batch_size`.
    pub batch_size: Option<u32>,
}

/// A [`QueueConsumer`] whose Queue has a Message to claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyConsumer {
    pub consumer: QueueConsumer,
    /// The Trigger's `batch_size`, or else its Queue's.
    pub batch_size: u32,
}
