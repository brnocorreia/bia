//! Approval Requests: an `agent` asks for a Version to be published and/or activated, and the
//! admin decides (ADR-0066).

mod model;
mod repository;
mod row;

pub use model::{
    ApprovalAction, ApprovalFilter, ApprovalRequest, ApprovalRequestId, ApprovalStatus,
    NewApprovalRequest,
};
pub use repository::{ApprovalRepository, PgApprovalRepository};
