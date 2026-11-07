use std::fmt;

use bia_core::dsl::WorkflowId;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::token::TokenId;
use crate::user::UserId;
use crate::version::VersionNumber;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApprovalRequestId(Uuid);

impl ApprovalRequestId {
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for ApprovalRequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What approving a request does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApprovalAction {
    Publish,
    PublishAndActivate,
    /// Makes an already Published Version Active; also how a rollback is requested.
    Activate,
}

impl ApprovalAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Publish => "publish",
            Self::PublishAndActivate => "publish_and_activate",
            Self::Activate => "activate",
        }
    }

    /// Whether approving publishes a Draft, so the request is about the Draft's content.
    pub fn publishes(self) -> bool {
        !matches!(self, Self::Activate)
    }

    pub fn activates(self) -> bool {
        !matches!(self, Self::Publish)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    /// The Draft was replaced after the request, so nobody approved what it now holds.
    Stale,
    Withdrawn,
}

impl ApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Stale => "stale",
            Self::Withdrawn => "withdrawn",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub id: ApprovalRequestId,
    pub workflow_id: WorkflowId,
    pub number: VersionNumber,
    pub action: ApprovalAction,
    pub note: Option<String>,
    /// SHA-256 of the Draft's Definition when the request was made; absent for `activate`.
    pub content_hash: Option<String>,
    pub status: ApprovalStatus,
    pub requested_by: UserId,
    pub requester_token: TokenId,
    /// The requesting Token's name, e.g. `claude-desktop`.
    pub requester_name: String,
    pub decided_by: Option<UserId>,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    /// When the status left `pending`.
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewApprovalRequest {
    pub workflow_id: WorkflowId,
    pub number: VersionNumber,
    pub action: ApprovalAction,
    pub note: Option<String>,
    pub content_hash: Option<String>,
    pub requested_by: UserId,
    pub requester_token: TokenId,
}

#[derive(Debug, Clone, Default)]
pub struct ApprovalFilter {
    pub status: Option<ApprovalStatus>,
    pub workflow: Option<WorkflowId>,
}
