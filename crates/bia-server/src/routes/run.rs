//! Runs: started by hand on the Active Version, listed, read with their Timeline, cancelled,
//! resumed and retried. `agent` Tokens may do all of it (ADR-0008); the Timeline entry is the
//! shape Dry Run mirrors (ADR-0045).

use axum::extract::State;
use axum::http::StatusCode;
use bia_core::dsl::{Definition, TriggerKind, WorkflowId};
use bia_core::validation::schema_violations;
use bia_engine::EngineError;
use bia_store::StoreError;
use bia_store::run::{PgRunRepository, Run, RunFilter, RunId, RunRepository, RunStatus};
use bia_store::timeline::{AttemptStatus, PgTimelineRepository, TimelineEntry, TimelineRepository};
use bia_store::version::{PgVersionRepository, VersionRepository};
use bia_store::workflow::{PgWorkflowRepository, WorkflowRepository};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::workflow::{acquire, store_error};
use crate::auth::{Agent, AuthErrors};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path, Query};

const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 200;
/// How many input violations an `invalid_run_input` message spells out.
const VIOLATIONS_SHOWN: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunStatusName {
    Pending,
    Running,
    Sleeping,
    Waiting,
    Succeeded,
    Failed,
    Cancelled,
}

impl From<RunStatus> for RunStatusName {
    fn from(status: RunStatus) -> Self {
        match status {
            RunStatus::Pending => Self::Pending,
            RunStatus::Running => Self::Running,
            RunStatus::Sleeping => Self::Sleeping,
            RunStatus::Waiting => Self::Waiting,
            RunStatus::Succeeded => Self::Succeeded,
            RunStatus::Failed => Self::Failed,
            RunStatus::Cancelled => Self::Cancelled,
        }
    }
}

impl From<RunStatusName> for RunStatus {
    fn from(status: RunStatusName) -> Self {
        match status {
            RunStatusName::Pending => Self::Pending,
            RunStatusName::Running => Self::Running,
            RunStatusName::Sleeping => Self::Sleeping,
            RunStatusName::Waiting => Self::Waiting,
            RunStatusName::Succeeded => Self::Succeeded,
            RunStatusName::Failed => Self::Failed,
            RunStatusName::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttemptStatusName {
    Succeeded,
    Failed,
    Skipped,
    /// Cut short by cancellation of the Run.
    Cancelled,
}

/// A Run without its input and Timeline, as listed.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RunSummary {
    id: Uuid,
    #[schema(value_type = String)]
    workflow_id: WorkflowId,
    /// The Version the Run is pinned to for its whole life, even if another becomes active.
    version: i32,
    status: RunStatusName,
    /// What started the Run: `{ "type": "manual", "id": <Trigger id> }`, `{ "type": "cron", ... }`
    /// and so on. A retried Run carries its original's `trigger` plus `retry_of`.
    trigger: Value,
    /// The Run this one is a retry of. Absent unless the Run was started by `retry_run`.
    retry_of: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// Absent until the Run is `succeeded`, `failed` or `cancelled`.
    finished_at: Option<DateTime<Utc>>,
    /// When cancellation was requested. A Run with this set that has not finished is still
    /// being stopped.
    cancel_requested_at: Option<DateTime<Utc>>,
}

/// One entry of a Run's Timeline: an Attempt of a Step, or, without `step_id`, something that
/// happened to the Run as a whole, such as input rejected before any Step ran.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct TimelineEntryInfo {
    /// Position in the Timeline, starting at 1. Entries are ordered by it.
    pub(crate) seq: i32,
    #[schema(value_type = Option<String>)]
    pub(crate) step_id: Option<String>,
    /// Which iteration of the enclosing loops, empty outside loops. Absent without `step_id`.
    pub(crate) iteration_path: Option<String>,
    /// Counts up across the Attempts of a Step, from 1, also across a resume. Absent without
    /// `step_id`.
    pub(crate) attempt: Option<i32>,
    #[schema(value_type = Option<String>, example = "http.request")]
    pub(crate) action: Option<String>,
    pub(crate) status: AttemptStatusName,
    /// The Params with their Templates resolved, except that any Template reading `secrets`
    /// is shown as written; for an entry about the Run, the Run input.
    pub(crate) input: Option<Value>,
    pub(crate) output: Option<Value>,
    /// `{ "class": "retryable" | "fatal", "message", "details"?, "code"? }`. `code` is a stable
    /// identifier when bia itself raised the error, e.g. `invalid_input` or `timeout`.
    pub(crate) error: Option<Value>,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) finished_at: DateTime<Utc>,
    pub(crate) duration_ms: i64,
}

/// A Run with its input and its Timeline.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RunInfo {
    #[serde(flatten)]
    summary: RunSummary,
    input: Value,
    /// Every entry so far, oldest first: one per Attempt, with its inputs, Output or error and
    /// duration.
    timeline: Vec<TimelineEntryInfo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RunList {
    /// Newest first.
    runs: Vec<RunSummary>,
    /// Pass as `after` to get the next page. Absent on the last page.
    next_after: Option<Uuid>,
}

/// A failed Run and the entry that failed it.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct FailedRun {
    #[serde(flatten)]
    summary: RunSummary,
    /// The last failed Timeline entry.
    failure: Option<TimelineEntryInfo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct FailureList {
    /// Failed Runs, newest first.
    runs: Vec<FailedRun>,
    /// Pass as `after` to get the next page. Absent on the last page.
    next_after: Option<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TriggerRequest {
    /// The Run input. Defaults to `{}`. Checked against the Active Version's `input_schema`.
    #[serde(default = "empty_input")]
    input: Value,
    /// The manual Trigger to start from. Defaults to the first manual Trigger of the Active
    /// Version.
    #[serde(default)]
    trigger_id: Option<String>,
}

fn empty_input() -> Value {
    json!({})
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListQuery {
    /// Only Runs of this Workflow.
    #[param(value_type = Option<String>)]
    workflow_id: Option<WorkflowId>,
    /// Only Runs in this status.
    status: Option<RunStatusName>,
    /// Only Runs created at or after this time (RFC 3339).
    created_from: Option<DateTime<Utc>>,
    /// Only Runs created before this time (RFC 3339).
    created_to: Option<DateTime<Utc>>,
    /// Return the page after this Run id (a previous page's `next_after`).
    after: Option<Uuid>,
    /// Page size, 1 to 200. Defaults to 50.
    limit: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub(crate) struct FailuresQuery {
    /// Only failures of this Workflow.
    #[param(value_type = Option<String>)]
    workflow_id: Option<WorkflowId>,
    /// Only Runs created at or after this time (RFC 3339).
    created_from: Option<DateTime<Utc>>,
    /// Only Runs created before this time (RFC 3339).
    created_to: Option<DateTime<Utc>>,
    /// Return the page after this Run id (a previous page's `next_after`).
    after: Option<Uuid>,
    /// Page size, 1 to 200. Defaults to 50.
    limit: Option<u32>,
}

fn runs() -> PgRunRepository {
    PgRunRepository
}

fn timelines() -> PgTimelineRepository {
    PgTimelineRepository
}

/// Starts a Run of the Workflow's Active Version from a manual Trigger. The input is checked
/// against the Version's `input_schema` first: input that does not match is refused with
/// `invalid_run_input`, and no Run is created. The Run is `pending` in the response and a
/// Worker picks it up. A Workflow with no Active Version is refused with `no_active_version`,
/// and one whose Active Version has no manual Trigger (or not the named one) with
/// `no_manual_trigger`.
#[utoipa::path(
    post,
    path = "/v1/workflows/{workflow_id}/runs",
    tag = "runs",
    security(("token" = [])),
    params(("workflow_id" = String, Path, description = "The Workflow's id.")),
    request_body(content = TriggerRequest, description = "The input; send `{}` for none."),
    responses(
        (status = CREATED, body = RunSummary),
        (status = BAD_REQUEST, description = "`malformed_request`.", body = ErrorBody),
        (status = NOT_FOUND, description = "`workflow_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`no_active_version` or `no_manual_trigger`.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_run_input`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn trigger_run(
    _: Agent,
    State(state): State<ApiState>,
    Path(workflow_id): Path<WorkflowId>,
    Json(request): Json<TriggerRequest>,
) -> Result<(StatusCode, Json<RunSummary>), ApiError> {
    let mut conn = acquire(&state).await?;
    let workflow = PgWorkflowRepository
        .get(&mut conn, &workflow_id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| store_error(StoreError::WorkflowNotFound(workflow_id.clone())))?;
    let version = workflow.active_version.ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::NoActiveVersion,
            format!("workflow `{workflow_id}` has no Active Version; activate a Published Version"),
        )
    })?;
    let stored = PgVersionRepository
        .get(&mut conn, &workflow_id, version)
        .await
        .map_err(store_error)?
        .ok_or_else(|| {
            store_error(StoreError::VersionNotFound {
                workflow: workflow_id.clone(),
                number: version,
            })
        })?;

    let trigger_id = manual_trigger(&stored.definition, request.trigger_id.as_deref())?;
    check_input(&stored.definition, &request.input)?;

    let trigger = json!({ "type": "manual", "id": trigger_id });
    let run = bia_engine::start_run(
        &mut conn,
        &workflow_id,
        version,
        request.input,
        trigger,
        state.clock.now(),
    )
    .await
    .map_err(engine_error)?;
    tracing::info!(run_id = %run.id, workflow = %workflow_id, version = version.get(), "run triggered");
    Ok((StatusCode::CREATED, Json(summary(run))))
}

/// Runs newest first, filtered by Workflow, status and creation time, paged with `after`
/// and `limit`.
#[utoipa::path(
    get,
    path = "/v1/runs",
    tag = "runs",
    security(("token" = [])),
    params(ListQuery),
    responses(
        (status = OK, body = RunList),
        (status = BAD_REQUEST, description = "`malformed_request`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn list_runs(
    _: Agent,
    State(state): State<ApiState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<RunList>, ApiError> {
    let mut conn = acquire(&state).await?;
    let filter = RunFilter {
        workflow_id: query.workflow_id,
        status: query.status.map(Into::into),
        created_from: query.created_from,
        created_to: query.created_to,
        ..RunFilter::default()
    };
    let (page, next_after) = page(&mut conn, filter, query.after, query.limit).await?;
    Ok(Json(RunList {
        runs: page.into_iter().map(summary).collect(),
        next_after,
    }))
}

/// The failure view for agents: `failed` Runs newest first, each with the Timeline entry that
/// failed it, so a cause can be read without fetching every Run. Filters and paging as in
/// `list_runs`.
#[utoipa::path(
    get,
    path = "/v1/runs/failures",
    tag = "runs",
    security(("token" = [])),
    params(FailuresQuery),
    responses(
        (status = OK, body = FailureList),
        (status = BAD_REQUEST, description = "`malformed_request`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn list_failures(
    _: Agent,
    State(state): State<ApiState>,
    Query(query): Query<FailuresQuery>,
) -> Result<Json<FailureList>, ApiError> {
    let mut conn = acquire(&state).await?;
    let filter = RunFilter {
        workflow_id: query.workflow_id,
        status: Some(RunStatus::Failed),
        created_from: query.created_from,
        created_to: query.created_to,
        ..RunFilter::default()
    };
    let (page, next_after) = page(&mut conn, filter, query.after, query.limit).await?;
    let ids: Vec<RunId> = page.iter().map(|run| run.id).collect();
    let mut failures = timelines()
        .last_failures(&mut conn, &ids)
        .await
        .map_err(store_error)?;
    let runs = page
        .into_iter()
        .map(|run| {
            let failure = failures
                .iter()
                .position(|entry| entry.run_id == run.id)
                .map(|at| failures.swap_remove(at))
                .map(entry);
            FailedRun {
                summary: summary(run),
                failure,
            }
        })
        .collect();
    Ok(Json(FailureList { runs, next_after }))
}

/// A Run with its input and full Timeline.
#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}",
    tag = "runs",
    security(("token" = [])),
    params(("run_id" = Uuid, Path, description = "The Run's id.")),
    responses(
        (status = OK, body = RunInfo),
        (status = NOT_FOUND, description = "`run_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn get_run(
    _: Agent,
    State(state): State<ApiState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<RunInfo>, ApiError> {
    let mut conn = acquire(&state).await?;
    let run = existing_run(&mut conn, run_id).await?;
    let timeline = timelines()
        .list(&mut conn, run.id)
        .await
        .map_err(store_error)?;
    let input = run.input.clone();
    Ok(Json(RunInfo {
        summary: summary(run),
        input,
        timeline: timeline.into_iter().map(entry).collect(),
    }))
}

/// Asks for the Run to stop. The response is 202: a Worker cancels the Attempt in flight and
/// the Run becomes `cancelled` a moment later (a sleeping Run at once), which `get_run` shows.
/// A Run that already finished is refused with `run_finished`.
#[utoipa::path(
    post,
    path = "/v1/runs/{run_id}/cancel",
    tag = "runs",
    security(("token" = [])),
    params(("run_id" = Uuid, Path, description = "The Run's id.")),
    responses(
        (status = ACCEPTED, body = RunSummary),
        (status = NOT_FOUND, description = "`run_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`run_finished`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn cancel_run(
    _: Agent,
    State(state): State<ApiState>,
    Path(run_id): Path<Uuid>,
) -> Result<(StatusCode, Json<RunSummary>), ApiError> {
    let mut conn = acquire(&state).await?;
    let id = RunId::from_uuid(run_id);
    let flagged = bia_engine::cancel_run(&mut conn, id, state.clock.now())
        .await
        .map_err(engine_error)?;
    if !flagged {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::RunFinished,
            format!("run {id} already finished"),
        ));
    }
    tracing::info!(run_id = %id, "run cancellation requested");
    let run = existing_run(&mut conn, run_id).await?;
    Ok((StatusCode::ACCEPTED, Json(summary(run))))
}

/// Resumes a `failed` Run from the Step that failed, on its own Version even if another is
/// active now. Steps that succeeded do not run again; the failed Step gets a fresh retry
/// budget and its Attempt numbers carry on. A Run that is not `failed`, or failed before any
/// Step ran (e.g. invalid input), is refused with `run_not_resumable`.
#[utoipa::path(
    post,
    path = "/v1/runs/{run_id}/resume",
    tag = "runs",
    security(("token" = [])),
    params(("run_id" = Uuid, Path, description = "The Run's id.")),
    responses(
        (status = OK, body = RunSummary),
        (status = NOT_FOUND, description = "`run_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`run_not_resumable`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn resume_run(
    _: Agent,
    State(state): State<ApiState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<RunSummary>, ApiError> {
    let mut conn = acquire(&state).await?;
    let run = bia_engine::resume_run(&mut conn, RunId::from_uuid(run_id), state.clock.now())
        .await
        .map_err(engine_error)?;
    tracing::info!(run_id = %run.id, "run resumed");
    Ok(Json(summary(run)))
}

/// Starts a new Run with the input and Version of a finished Run. The new Run's `trigger` is
/// the original's plus `retry_of`, the original's id. Re-running on a newer Version means
/// triggering the Workflow instead. A Run that has not finished is refused with
/// `run_not_finished`.
#[utoipa::path(
    post,
    path = "/v1/runs/{run_id}/retry",
    tag = "runs",
    security(("token" = [])),
    params(("run_id" = Uuid, Path, description = "The Run's id.")),
    responses(
        (status = CREATED, body = RunSummary),
        (status = NOT_FOUND, description = "`run_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`run_not_finished`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn retry_run(
    _: Agent,
    State(state): State<ApiState>,
    Path(run_id): Path<Uuid>,
) -> Result<(StatusCode, Json<RunSummary>), ApiError> {
    let mut conn = acquire(&state).await?;
    let original = existing_run(&mut conn, run_id).await?;
    if !original.status.is_finished() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::RunNotFinished,
            format!(
                "run {} is {}; only a finished Run is retried",
                original.id,
                original.status.as_str()
            ),
        ));
    }
    let mut trigger = original.trigger;
    match &mut trigger {
        Value::Object(fields) => {
            fields.insert("retry_of".to_owned(), json!(original.id.as_uuid()));
        }
        other => *other = json!({ "retry_of": original.id.as_uuid() }),
    }
    let run = bia_engine::start_run(
        &mut conn,
        &original.workflow_id,
        original.version,
        original.input,
        trigger,
        state.clock.now(),
    )
    .await
    .map_err(engine_error)?;
    tracing::info!(run_id = %run.id, retry_of = %original.id, "run retried");
    Ok((StatusCode::CREATED, Json(summary(run))))
}

/// The id of the manual Trigger to start from.
fn manual_trigger<'a>(
    definition: &'a Definition,
    wanted: Option<&str>,
) -> Result<&'a str, ApiError> {
    let mut manual = definition
        .triggers
        .iter()
        .filter(|trigger| trigger.kind == TriggerKind::Manual)
        .map(|trigger| trigger.id.as_str());
    let found = match wanted {
        Some(wanted) => manual.find(|id| *id == wanted),
        None => manual.next(),
    };
    found.ok_or_else(|| {
        let message = match wanted {
            Some(wanted) => format!("the Active Version has no manual Trigger `{wanted}`"),
            None => "the Active Version has no manual Trigger".to_owned(),
        };
        ApiError::new(StatusCode::CONFLICT, ErrorCode::NoManualTrigger, message)
    })
}

fn check_input(definition: &Definition, input: &Value) -> Result<(), ApiError> {
    let Some(schema) = &definition.input_schema else {
        return Ok(());
    };
    let violations = match schema_violations(&Value::Object(schema.clone()), input) {
        Ok(violations) => violations,
        Err(error) => return Err(ApiError::internal(&error)),
    };
    if violations.is_empty() {
        return Ok(());
    }
    // Violations never echo the input, which may be sensitive.
    let shown: Vec<String> = violations
        .iter()
        .take(VIOLATIONS_SHOWN)
        .map(|v| format!("`{}`: {}", v.path, v.message))
        .collect();
    let mut message = format!(
        "the input does not match the input_schema ({} violation(s)): {}",
        violations.len(),
        shown.join("; ")
    );
    if violations.len() > VIOLATIONS_SHOWN {
        message.push_str("; and more");
    }
    Err(ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::InvalidRunInput,
        message,
    ))
}

/// One page of `filter` after `after`, and the cursor for the next one.
async fn page(
    conn: &mut PgConnection,
    mut filter: RunFilter,
    after: Option<Uuid>,
    limit: Option<u32>,
) -> Result<(Vec<Run>, Option<Uuid>), ApiError> {
    let limit = limit.unwrap_or(DEFAULT_PAGE);
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err(ApiError::malformed_request(format!(
            "`limit` is between 1 and {MAX_PAGE}"
        )));
    }
    filter.after = after.map(RunId::from_uuid);
    // One more than asked for tells whether another page follows.
    filter.limit = limit + 1;
    let mut found = runs().list(conn, &filter).await.map_err(store_error)?;
    let size = limit as usize;
    let more = found.len() > size;
    found.truncate(size);
    let next_after = more
        .then(|| found.last().map(|run| run.id.as_uuid()))
        .flatten();
    Ok((found, next_after))
}

async fn existing_run(conn: &mut PgConnection, id: Uuid) -> Result<Run, ApiError> {
    let id = RunId::from_uuid(id);
    runs()
        .get(conn, id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| run_not_found(id))
}

fn run_not_found(id: RunId) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        ErrorCode::RunNotFound,
        format!("run {id} not found"),
    )
}

fn summary(run: Run) -> RunSummary {
    let retry_of = run
        .trigger
        .get("retry_of")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    RunSummary {
        id: run.id.as_uuid(),
        workflow_id: run.workflow_id,
        version: run.version.get(),
        status: run.status.into(),
        trigger: run.trigger,
        retry_of,
        created_at: run.created_at,
        updated_at: run.updated_at,
        finished_at: run.finished_at,
        cancel_requested_at: run.cancel_requested_at,
    }
}

pub(super) fn entry(entry: TimelineEntry) -> TimelineEntryInfo {
    let duration_ms = entry.duration().num_milliseconds();
    let (step_id, iteration_path, attempt, action) = match entry.step {
        Some(step) => (
            Some(step.step_id.as_str().to_owned()),
            Some(step.iteration_path),
            Some(step.attempt),
            Some(step.action.as_str().to_owned()),
        ),
        None => (None, None, None, None),
    };
    TimelineEntryInfo {
        seq: entry.seq,
        step_id,
        iteration_path,
        attempt,
        action,
        status: match entry.status {
            AttemptStatus::Succeeded => AttemptStatusName::Succeeded,
            AttemptStatus::Failed => AttemptStatusName::Failed,
            AttemptStatus::Skipped => AttemptStatusName::Skipped,
            AttemptStatus::Cancelled => AttemptStatusName::Cancelled,
        },
        input: entry.input,
        output: entry.output,
        error: entry.error,
        started_at: entry.started_at,
        finished_at: entry.finished_at,
        duration_ms,
    }
}

fn engine_error(error: EngineError) -> ApiError {
    match error {
        EngineError::RunNotFound(id) | EngineError::Store(StoreError::RunNotFound(id)) => {
            run_not_found(id)
        }
        // The reason names only the Run's status or that it never ran a Step.
        EngineError::NotResumable { .. } => ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::RunNotResumable,
            error.to_string(),
        ),
        EngineError::Store(error) => store_error(error),
        other => ApiError::internal(&other),
    }
}
