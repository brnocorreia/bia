use bia_core::dsl::{ActionName, StepId};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

use super::RunId;

/// How an Attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptStatus {
    Succeeded,
    Failed,
    /// The Step's `if` was false, so its Action never ran.
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

/// One Attempt to record. Entries reach operators and agents verbatim, so inputs, Outputs
/// and errors must never contain a resolved Secret.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTimelineEntry {
    pub step_id: StepId,
    /// Which iteration of the enclosing loops, as in the Idempotency Key; empty outside loops.
    pub iteration_path: String,
    /// 1 for the first Attempt, then 2, 3… on retries.
    pub attempt: i32,
    pub action: ActionName,
    pub status: AttemptStatus,
    /// The resolved Params, without Secrets.
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<Value>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

/// One recorded Attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineEntry {
    pub run_id: RunId,
    /// Position in the Run's Timeline: 1, 2, 3…
    pub seq: i32,
    pub step_id: StepId,
    pub iteration_path: String,
    pub attempt: i32,
    pub action: ActionName,
    pub status: AttemptStatus,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<Value>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

impl TimelineEntry {
    /// How long the Attempt took.
    pub fn duration(&self) -> TimeDelta {
        self.finished_at - self.started_at
    }
}
