//! Approval Requests: an `agent` asks for a Version to be published and/or activated, and the
//! admin approves or rejects (ADR-0066, amending ADR-0008). Approving performs the change as the
//! deciding User, in the transaction that records the decision.

use axum::extract::State;
use axum::http::StatusCode;
use bia_core::dsl::{Definition, WorkflowId};
use bia_store::StoreError;
use bia_store::approval::{
    ApprovalAction, ApprovalFilter, ApprovalRepository, ApprovalRequest, ApprovalRequestId,
    ApprovalStatus, NewApprovalRequest, PgApprovalRepository,
};
use bia_store::version::VersionState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::workflow::{
    acquire, activate, definition_value, existing_version, existing_workflow, publish_draft,
    publishable_draft, store_error, version_number,
};
use crate::auth::{Admin, Agent, AuthErrors};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path, Query};

/// Longest note or comment, in characters.
const MAX_TEXT_LEN: usize = 2_000;

/// What approving the request does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActionName {
    /// Publish the Draft.
    Publish,
    /// Publish the Draft and make it the Active Version.
    PublishAndActivate,
    /// Make an already Published Version the Active Version, which is also how a rollback is
    /// requested.
    Activate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StatusName {
    /// Waiting for the admin.
    Pending,
    /// The admin approved it and the change was made.
    Approved,
    /// The admin rejected it; `comment` may say why.
    Rejected,
    /// The Draft was replaced after the request, so it can no longer be approved.
    Stale,
    /// The requester, or a newer request for the same Workflow, withdrew it.
    Withdrawn,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewApprovalRequestBody {
    #[schema(value_type = String, example = "onboarding-email")]
    workflow_id: WorkflowId,
    /// The Version's number: a Draft to publish, or a Published Version to activate.
    number: i32,
    action: ActionName,
    /// Why the change should go ahead, for the admin. Up to 2000 characters.
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DecisionBody {
    /// Why, for the requester. Up to 2000 characters.
    #[serde(default)]
    comment: Option<String>,
}

/// An Approval Request, with what a review page shows: the Workflow, the Version, the action,
/// who asked and why, how long ago, and how it was decided.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ApprovalRequestInfo {
    id: Uuid,
    #[schema(value_type = String, example = "onboarding-email")]
    workflow_id: WorkflowId,
    /// The Version the request is about.
    number: i32,
    action: ActionName,
    note: Option<String>,
    status: StatusName,
    /// The name of the Token the request was made with, e.g. `claude-desktop`.
    requester_name: String,
    /// The User the requesting Token acts for.
    requested_by: Uuid,
    requester_token_id: Uuid,
    /// SHA-256 of the Draft's Definition when the request was made, as hex. Absent for
    /// `activate`.
    content_hash: Option<String>,
    /// The User who approved or rejected it.
    decided_by: Option<Uuid>,
    /// The admin's comment on the decision.
    comment: Option<String>,
    /// The request's age is now minus this.
    created_at: DateTime<Utc>,
    /// When the status left `pending`, whatever the reason.
    closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ApprovalRequestList {
    /// Newest first.
    approval_requests: Vec<ApprovalRequestInfo>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListQuery {
    /// Only requests with this status.
    status: Option<StatusName>,
    /// Only requests for this Workflow.
    workflow_id: Option<String>,
}

fn approvals() -> PgApprovalRepository {
    PgApprovalRepository
}

/// Asks for a Version to be published and/or activated. A Draft target must pass the checks
/// publishing makes, or the request fails with `invalid_definition` and the Diagnostics and
/// nothing is created. `activate` needs a Published Version. The Workflow's earlier pending
/// request becomes `withdrawn`. Replacing the Draft later makes the request `stale`. Needs an
/// `agent` or `admin` Token; an admin decides it with `approve` or `reject`.
#[utoipa::path(
    post,
    path = "/v1/approval-requests",
    tag = "approvals",
    security(("token" = [])),
    request_body = NewApprovalRequestBody,
    responses(
        (status = CREATED, body = ApprovalRequestInfo),
        (status = BAD_REQUEST, description = "`malformed_request`.", body = ErrorBody),
        (status = NOT_FOUND, description = "`workflow_not_found` or `version_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`version_published` for a Published Version asked to be published, or `version_not_published` for a Draft asked to be activated.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_definition`: the Draft cannot be published; `diagnostics` says why.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn create_approval_request(
    Agent(caller): Agent,
    State(state): State<ApiState>,
    Json(body): Json<NewApprovalRequestBody>,
) -> Result<(StatusCode, Json<ApprovalRequestInfo>), ApiError> {
    let number = version_number(body.number)?;
    let note = text("note", body.note)?;
    let action = action(body.action);
    let mut conn = acquire(&state).await?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    existing_workflow(&mut tx, &body.workflow_id).await?;
    let content_hash = if action.publishes() {
        let (draft, _) = publishable_draft(&state, &mut tx, &body.workflow_id, number).await?;
        Some(content_hash(&draft.definition))
    } else {
        let version = existing_version(&mut tx, &body.workflow_id, number).await?;
        if version.state != VersionState::Published {
            return Err(store_error(StoreError::VersionNotPublished {
                workflow: body.workflow_id,
                number,
            }));
        }
        None
    };
    let created = approvals()
        .create(
            &mut tx,
            &NewApprovalRequest {
                workflow_id: body.workflow_id,
                number,
                action,
                note,
                content_hash,
                requested_by: caller.user_id,
                requester_token: caller.id,
            },
            state.clock.now(),
        )
        .await
        .map_err(store_error)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    tracing::info!(
        request = %created.id,
        workflow = %created.workflow_id,
        version = created.number.get(),
        action = created.action.as_str(),
        "approval requested"
    );
    Ok((StatusCode::CREATED, Json(created.into())))
}

/// Approval Requests, newest first, optionally of one status or Workflow.
#[utoipa::path(
    get,
    path = "/v1/approval-requests",
    tag = "approvals",
    security(("token" = [])),
    params(ListQuery),
    responses(
        (status = OK, body = ApprovalRequestList),
        (status = BAD_REQUEST, description = "`malformed_request`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn list_approval_requests(
    _: Agent,
    State(state): State<ApiState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApprovalRequestList>, ApiError> {
    let workflow = query
        .workflow_id
        .map(|id| {
            WorkflowId::new(id)
                .map_err(|e| ApiError::malformed_request(format!("`workflow_id`: {e}")))
        })
        .transpose()?;
    let filter = ApprovalFilter {
        status: query.status.map(status),
        workflow,
    };
    let mut conn = acquire(&state).await?;
    let list = approvals()
        .list(&mut conn, &filter)
        .await
        .map_err(store_error)?;
    Ok(Json(ApprovalRequestList {
        approval_requests: list.into_iter().map(Into::into).collect(),
    }))
}

/// One Approval Request.
#[utoipa::path(
    get,
    path = "/v1/approval-requests/{id}",
    tag = "approvals",
    security(("token" = [])),
    params(("id" = Uuid, Path, description = "The request's id.")),
    responses(
        (status = OK, body = ApprovalRequestInfo),
        (status = NOT_FOUND, description = "`approval_request_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn get_approval_request(
    _: Agent,
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApprovalRequestInfo>, ApiError> {
    let id = ApprovalRequestId::from_uuid(id);
    let mut conn = acquire(&state).await?;
    let request = approvals()
        .get(&mut conn, id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| store_error(StoreError::ApprovalRequestNotFound(id)))?;
    Ok(Json(request.into()))
}

/// Withdraws a pending request. Needs an `agent` or `admin` Token.
#[utoipa::path(
    post,
    path = "/v1/approval-requests/{id}/withdraw",
    tag = "approvals",
    security(("token" = [])),
    params(("id" = Uuid, Path, description = "The request's id.")),
    responses(
        (status = OK, body = ApprovalRequestInfo),
        (status = NOT_FOUND, description = "`approval_request_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`approval_request_not_pending`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn withdraw_approval_request(
    _: Agent,
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApprovalRequestInfo>, ApiError> {
    let mut conn = acquire(&state).await?;
    let request = approvals()
        .withdraw(
            &mut conn,
            ApprovalRequestId::from_uuid(id),
            state.clock.now(),
        )
        .await
        .map_err(store_error)?;
    Ok(Json(request.into()))
}

/// Approves a pending request and makes its change as the deciding User: publishes the Draft
/// and/or activates the Version, all in one transaction with the status change, so when the
/// change fails nothing happens and the request stays pending. A Draft that was replaced makes
/// the request `stale` and unapprovable. Needs an `admin` Token; the body may be `{}`.
#[utoipa::path(
    post,
    path = "/v1/approval-requests/{id}/approve",
    tag = "approvals",
    security(("token" = [])),
    params(("id" = Uuid, Path, description = "The request's id.")),
    request_body = DecisionBody,
    responses(
        (status = OK, body = ApprovalRequestInfo),
        (status = NOT_FOUND, description = "`approval_request_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`approval_request_not_pending`, `version_published`, `version_not_published` or `version_changed`.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_definition`: the Draft no longer passes publishing's checks.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn approve_approval_request(
    Admin(decider): Admin,
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<DecisionBody>,
) -> Result<Json<ApprovalRequestInfo>, ApiError> {
    let id = ApprovalRequestId::from_uuid(id);
    let comment = text("comment", body.comment)?;
    let mut conn = acquire(&state).await?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let request = approvals()
        .get(&mut tx, id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| store_error(StoreError::ApprovalRequestNotFound(id)))?;
    if request.status != ApprovalStatus::Pending {
        return Err(store_error(StoreError::ApprovalRequestNotPending(id)));
    }
    make_change(&state, &mut tx, &request).await?;
    let decided = approvals()
        .decide(
            &mut tx,
            id,
            true,
            decider.user_id,
            comment.as_deref(),
            state.clock.now(),
        )
        .await
        .map_err(store_error)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    tracing::info!(request = %id, workflow = %decided.workflow_id, "approval request approved");
    Ok(Json(decided.into()))
}

/// Rejects a pending request; the Version stays as it is. Needs an `admin` Token; the body may
/// be `{}`.
#[utoipa::path(
    post,
    path = "/v1/approval-requests/{id}/reject",
    tag = "approvals",
    security(("token" = [])),
    params(("id" = Uuid, Path, description = "The request's id.")),
    request_body = DecisionBody,
    responses(
        (status = OK, body = ApprovalRequestInfo),
        (status = NOT_FOUND, description = "`approval_request_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`approval_request_not_pending`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn reject_approval_request(
    Admin(decider): Admin,
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<DecisionBody>,
) -> Result<Json<ApprovalRequestInfo>, ApiError> {
    let comment = text("comment", body.comment)?;
    let mut conn = acquire(&state).await?;
    let decided = approvals()
        .decide(
            &mut conn,
            ApprovalRequestId::from_uuid(id),
            false,
            decider.user_id,
            comment.as_deref(),
            state.clock.now(),
        )
        .await
        .map_err(store_error)?;
    tracing::info!(request = %decided.id, workflow = %decided.workflow_id, "approval request rejected");
    Ok(Json(decided.into()))
}

/// Publishes and/or activates what the request asked for, inside the caller's transaction.
async fn make_change(
    state: &ApiState,
    tx: &mut PgConnection,
    request: &ApprovalRequest,
) -> Result<(), ApiError> {
    if request.action.publishes() {
        let (published, _) = publish_draft(state, tx, &request.workflow_id, request.number).await?;
        // `stale` normally keeps this from happening; it guards a replace that committed
        // between the read of the request and the publish.
        if request.content_hash.as_deref() != Some(content_hash(&published.definition).as_str()) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::VersionChanged,
                "the Draft is not what was requested; ask for approval again",
            ));
        }
    }
    if request.action.activates() {
        activate(tx, &request.workflow_id, request.number).await?;
    }
    Ok(())
}

/// SHA-256 of the Definition in its canonical JSON form, as hex.
fn content_hash(definition: &Definition) -> String {
    let bytes = serde_json::to_vec(&definition_value(definition))
        .unwrap_or_else(|_| unreachable!("a JSON value serialises"));
    hex::encode(Sha256::digest(bytes))
}

fn text(field: &str, value: Option<String>) -> Result<Option<String>, ApiError> {
    match value.filter(|v| !v.trim().is_empty()) {
        Some(v) if v.chars().count() > MAX_TEXT_LEN => Err(ApiError::malformed_request(format!(
            "`{field}` is at most {MAX_TEXT_LEN} characters"
        ))),
        other => Ok(other),
    }
}

fn action(name: ActionName) -> ApprovalAction {
    match name {
        ActionName::Publish => ApprovalAction::Publish,
        ActionName::PublishAndActivate => ApprovalAction::PublishAndActivate,
        ActionName::Activate => ApprovalAction::Activate,
    }
}

fn status(name: StatusName) -> ApprovalStatus {
    match name {
        StatusName::Pending => ApprovalStatus::Pending,
        StatusName::Approved => ApprovalStatus::Approved,
        StatusName::Rejected => ApprovalStatus::Rejected,
        StatusName::Stale => ApprovalStatus::Stale,
        StatusName::Withdrawn => ApprovalStatus::Withdrawn,
    }
}

impl From<ApprovalRequest> for ApprovalRequestInfo {
    fn from(request: ApprovalRequest) -> Self {
        Self {
            id: request.id.as_uuid(),
            workflow_id: request.workflow_id,
            number: request.number.get(),
            action: match request.action {
                ApprovalAction::Publish => ActionName::Publish,
                ApprovalAction::PublishAndActivate => ActionName::PublishAndActivate,
                ApprovalAction::Activate => ActionName::Activate,
            },
            note: request.note,
            status: match request.status {
                ApprovalStatus::Pending => StatusName::Pending,
                ApprovalStatus::Approved => StatusName::Approved,
                ApprovalStatus::Rejected => StatusName::Rejected,
                ApprovalStatus::Stale => StatusName::Stale,
                ApprovalStatus::Withdrawn => StatusName::Withdrawn,
            },
            requester_name: request.requester_name,
            requested_by: request.requested_by.as_uuid(),
            requester_token_id: request.requester_token.as_uuid(),
            content_hash: request.content_hash,
            decided_by: request.decided_by.map(|user| user.as_uuid()),
            comment: request.comment,
            created_at: request.created_at,
            closed_at: request.closed_at,
        }
    }
}
