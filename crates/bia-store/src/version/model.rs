use std::fmt;

use bia_core::dsl::{Definition, WorkflowId};
use chrono::{DateTime, Utc};

/// A Version's number within its Workflow: 1, 2, 3…
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VersionNumber(i32);

impl VersionNumber {
    /// Returns the number if it is positive.
    pub fn new(number: i32) -> Option<Self> {
        (number > 0).then_some(Self(number))
    }

    pub fn get(self) -> i32 {
        self.0
    }
}

impl fmt::Display for VersionNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Whether a Version can still change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VersionState {
    Draft,
    Published,
}

impl VersionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
        }
    }
}

/// One immutable snapshot of a Workflow's Definition (ADR-0009).
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub workflow_id: WorkflowId,
    pub number: VersionNumber,
    pub definition: Definition,
    pub state: VersionState,
    /// When the Version was published; `None` for a Draft.
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
