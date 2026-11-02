use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{ApprovalAction, ApprovalRequest, ApprovalRequestId, ApprovalStatus};
use crate::StoreError;
use crate::token::TokenId;
use crate::user::UserId;
use crate::version::version_number;
use crate::workflow::workflow_id;

#[derive(Debug)]
pub(crate) struct ApprovalRow {
    pub(crate) id: Uuid,
    pub(crate) workflow_id: String,
    pub(crate) number: i32,
    pub(crate) action: String,
    pub(crate) note: Option<String>,
    pub(crate) content_hash: Option<String>,
    pub(crate) status: String,
    pub(crate) requested_by: Uuid,
    pub(crate) requester_token: Uuid,
    pub(crate) requester_name: String,
    pub(crate) decided_by: Option<Uuid>,
    pub(crate) comment: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) closed_at: Option<DateTime<Utc>>,
}

impl TryFrom<ApprovalRow> for ApprovalRequest {
    type Error = StoreError;

    fn try_from(row: ApprovalRow) -> Result<Self, StoreError> {
        let action = match row.action.as_str() {
            "publish" => ApprovalAction::Publish,
            "publish_and_activate" => ApprovalAction::PublishAndActivate,
            "activate" => ApprovalAction::Activate,
            other => return Err(StoreError::invalid("approval action", other)),
        };
        Ok(Self {
            id: ApprovalRequestId::from_uuid(row.id),
            workflow_id: workflow_id(row.workflow_id)?,
            number: version_number(row.number)?,
            action,
            note: row.note,
            content_hash: row.content_hash,
            status: status(&row.status)?,
            requested_by: UserId::from_uuid(row.requested_by),
            requester_token: TokenId::from_uuid(row.requester_token),
            requester_name: row.requester_name,
            decided_by: row.decided_by.map(UserId::from_uuid),
            comment: row.comment,
            created_at: row.created_at,
            closed_at: row.closed_at,
        })
    }
}

fn status(status: &str) -> Result<ApprovalStatus, StoreError> {
    match status {
        "pending" => Ok(ApprovalStatus::Pending),
        "approved" => Ok(ApprovalStatus::Approved),
        "rejected" => Ok(ApprovalStatus::Rejected),
        "stale" => Ok(ApprovalStatus::Stale),
        "withdrawn" => Ok(ApprovalStatus::Withdrawn),
        other => Err(StoreError::invalid("approval status", other)),
    }
}
