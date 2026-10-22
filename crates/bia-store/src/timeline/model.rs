use bia_core::dsl::{ActionName, StepId};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

use crate::run::RunId;

/// How an Attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptStatus {
    Succeeded,
    Failed,
    Skipped,
}

impl AttemptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// Which Attempt of which Step an entry records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepAttempt {
    pub step_id: StepId,
    /// Which iteration of the enclosing loops, as in the Idempotency Key; empty outside loops.
    pub iteration_path: String,
    pub attempt: i32,
    pub action: ActionName,
}

/// One entry to record. Entries reach operators and agents verbatim, so inputs, Outputs and
/// errors must never contain a resolved Secret.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTimelineEntry {
    /// The Attempt recorded, or `None` for an entry about the Run as a whole, such as input
    /// rejected before any Step ran.
    pub step: Option<StepAttempt>,
    pub status: AttemptStatus,
    /// The resolved Params, without Secrets; the Run input for an entry about the Run.
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<Value>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineEntry {
    pub run_id: RunId,
    pub seq: i32,
    pub step: Option<StepAttempt>,
    pub status: AttemptStatus,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<Value>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

impl TimelineEntry {
    pub fn duration(&self) -> TimeDelta {
        self.finished_at - self.started_at
    }
}
