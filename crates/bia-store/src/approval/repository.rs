use std::future::Future;

use bia_core::dsl::WorkflowId;
use chrono::{DateTime, Utc};
use sqlx::{Acquire, PgConnection};

use super::row::ApprovalRow;
use super::{
    ApprovalFilter, ApprovalRequest, ApprovalRequestId, ApprovalStatus, NewApprovalRequest,
};
use crate::StoreError;
use crate::user::UserId;
use crate::version::VersionNumber;

/// Approval Requests and their status changes (ADR-0066). Every method takes a connection so
/// a decision commits with the publish it causes, and a Draft's replacement with the `stale`
/// it causes (ADR-0010); `now` comes from the caller's `Clock` (ADR-0022).
pub trait ApprovalRepository {
    /// Creates a pending request, withdrawing the Workflow's earlier pending one.
    fn create(
        &self,
        conn: &mut PgConnection,
        request: &NewApprovalRequest,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<ApprovalRequest, StoreError>> + Send;

    fn get(
        &self,
        conn: &mut PgConnection,
        id: ApprovalRequestId,
    ) -> impl Future<Output = Result<Option<ApprovalRequest>, StoreError>> + Send;

    /// Newest first.
    fn list(
        &self,
        conn: &mut PgConnection,
        filter: &ApprovalFilter,
    ) -> impl Future<Output = Result<Vec<ApprovalRequest>, StoreError>> + Send;

    /// Fails with [`StoreError::ApprovalRequestNotFound`] or, once the request is no longer
    /// pending, [`StoreError::ApprovalRequestNotPending`].
    fn withdraw(
        &self,
        conn: &mut PgConnection,
        id: ApprovalRequestId,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<ApprovalRequest, StoreError>> + Send;

    /// Marks a pending request `approved` or `rejected`, with the same failures as
    /// [`withdraw`](Self::withdraw). Approving does not publish anything: the caller does, in
    /// the same transaction.
    fn decide(
        &self,
        conn: &mut PgConnection,
        id: ApprovalRequestId,
        approve: bool,
        decider: UserId,
        comment: Option<&str>,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<ApprovalRequest, StoreError>> + Send;

    /// Marks the pending request about this Draft `stale`. Returns whether there was one.
    fn mark_stale(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        number: VersionNumber,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgApprovalRepository;

impl ApprovalRepository for PgApprovalRepository {
    async fn create(
        &self,
        conn: &mut PgConnection,
        request: &NewApprovalRequest,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRequest, StoreError> {
        let mut tx = conn.begin().await?;
        sqlx::query_file!(
            "src/approval/queries/withdraw_pending.sql",
            request.workflow_id.as_str(),
            now,
        )
        .execute(&mut *tx)
        .await?;
        let id = sqlx::query_file_scalar!(
            "src/approval/queries/insert.sql",
            request.workflow_id.as_str(),
            request.number.get(),
            request.action.as_str(),
            request.note,
            request.content_hash,
            request.requested_by.as_uuid(),
            request.requester_token.as_uuid(),
            now,
        )
        .fetch_one(&mut *tx)
        .await?;
        let created = fetch(&mut tx, ApprovalRequestId::from_uuid(id)).await?;
        tx.commit().await?;
        created.ok_or_else(|| StoreError::invalid("approval request", "missing after insert"))
    }

    async fn get(
        &self,
        conn: &mut PgConnection,
        id: ApprovalRequestId,
    ) -> Result<Option<ApprovalRequest>, StoreError> {
        fetch(conn, id).await
    }

    async fn list(
        &self,
        conn: &mut PgConnection,
        filter: &ApprovalFilter,
    ) -> Result<Vec<ApprovalRequest>, StoreError> {
        sqlx::query_file_as!(
            ApprovalRow,
            "src/approval/queries/list.sql",
            filter.status.map(ApprovalStatus::as_str),
            filter.workflow.as_ref().map(WorkflowId::as_str),
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(ApprovalRequest::try_from)
        .collect()
    }

    async fn withdraw(
        &self,
        conn: &mut PgConnection,
        id: ApprovalRequestId,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRequest, StoreError> {
        close(conn, id, ApprovalStatus::Withdrawn, None, None, now).await
    }

    async fn decide(
        &self,
        conn: &mut PgConnection,
        id: ApprovalRequestId,
        approve: bool,
        decider: UserId,
        comment: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRequest, StoreError> {
        let status = if approve {
            ApprovalStatus::Approved
        } else {
            ApprovalStatus::Rejected
        };
        close(conn, id, status, Some(decider), comment, now).await
    }

    async fn mark_stale(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        number: VersionNumber,
        now: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query_file!(
            "src/approval/queries/mark_stale.sql",
            workflow.as_str(),
            number.get(),
            now,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();
        Ok(updated > 0)
    }
}

async fn fetch(
    conn: &mut PgConnection,
    id: ApprovalRequestId,
) -> Result<Option<ApprovalRequest>, StoreError> {
    sqlx::query_file_as!(ApprovalRow, "src/approval/queries/get.sql", id.as_uuid())
        .fetch_optional(&mut *conn)
        .await?
        .map(ApprovalRequest::try_from)
        .transpose()
}

async fn close(
    conn: &mut PgConnection,
    id: ApprovalRequestId,
    status: ApprovalStatus,
    decider: Option<UserId>,
    comment: Option<&str>,
    now: DateTime<Utc>,
) -> Result<ApprovalRequest, StoreError> {
    let closed = sqlx::query_file_scalar!(
        "src/approval/queries/close.sql",
        id.as_uuid(),
        status.as_str(),
        decider.map(UserId::as_uuid),
        comment,
        now,
    )
    .fetch_optional(&mut *conn)
    .await?;
    match (closed, fetch(conn, id).await?) {
        (Some(_), Some(request)) => Ok(request),
        (None, Some(_)) => Err(StoreError::ApprovalRequestNotPending(id)),
        _ => Err(StoreError::ApprovalRequestNotFound(id)),
    }
}
