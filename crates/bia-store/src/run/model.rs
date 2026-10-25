use std::fmt;

use bia_core::dsl::WorkflowId;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::version::VersionNumber;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(Uuid);

impl RunId {
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Where a Run is in its lifecycle: `pending → running → (sleeping | waiting) → running →
/// succeeded | failed | cancelled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunStatus {
    Pending,
    Running,
    Sleeping,
    Waiting,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Sleeping => "sleeping",
            Self::Waiting => "waiting",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// The persisted state of a Run after a transition. The interpreter owns what goes inside.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Checkpoint {
    pub cursor: Vec<Value>,
    pub state: Value,
}

/// One execution of a Version, pinned to it for its whole life (ADR-0009).
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub id: RunId,
    pub workflow_id: WorkflowId,
    pub version: VersionNumber,
    pub status: RunStatus,
    pub input: Value,
    /// What started the Run, readable as `trigger` in Expressions.
    pub trigger: Value,
    pub checkpoint: Checkpoint,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// When cancellation was first requested. A Run flagged but not yet finished is still being
    /// stopped: the Worker running it ends it `cancelled` at its next transition.
    pub cancel_requested_at: Option<DateTime<Utc>>,
}

/// What a new Run starts from.
#[derive(Debug, Clone, PartialEq)]
pub struct NewRun {
    pub workflow_id: WorkflowId,
    pub version: VersionNumber,
    pub input: Value,
    pub trigger: Value,
    pub checkpoint: Checkpoint,
}

/// Which Runs a listing returns, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct RunFilter {
    pub workflow_id: Option<WorkflowId>,
    pub status: Option<RunStatus>,
    /// Only Runs created at or after this time.
    pub created_from: Option<DateTime<Utc>>,
    /// Only Runs created before this time.
    pub created_to: Option<DateTime<Utc>>,
    pub after: Option<RunId>,
    pub limit: u32,
}

impl Default for RunFilter {
    fn default() -> Self {
        Self {
            workflow_id: None,
            status: None,
            created_from: None,
            created_to: None,
            after: None,
            limit: 50,
        }
    }
}
