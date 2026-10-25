//! Workflows and their Versions: Drafts are created and replaced, validated, diffed and read
//! by `agent` Tokens; publishing and activating, which includes rolling back, need `admin`
//! (ADR-0008, ADR-0044).

use axum::extract::State;
use axum::http::StatusCode;
use bia_core::dsl::{Definition, WorkflowId};
use bia_core::validation::{Diagnostic, KnownResources, Target, Validator};
use bia_store::StoreError;
use bia_store::version::{
    PgVersionRepository, Version, VersionNumber, VersionRepository, VersionState,
};
use bia_store::workflow::{PgWorkflowRepository, Workflow, WorkflowRepository};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;
use utoipa::{IntoParams, ToSchema};

use crate::auth::{Admin, Agent, AuthErrors};
use crate::error::DiagnosticSchema;
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path, Query};

/// A Workflow and the Version new Runs and Schedules use.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WorkflowInfo {
    #[schema(value_type = String, example = "onboarding-email")]
    id: WorkflowId,
    /// The Published Version new Runs start on and whose Triggers fire. Absent until a Version
    /// is activated.
    active_version: Option<i32>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WorkflowList {
    /// Every Workflow, ordered by id.
    workflows: Vec<WorkflowInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StateName {
    /// Editable by replacement; starts no Runs.
    Draft,
    /// Frozen for good.
    Published,
}

/// A Version without its Definition, as listed.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct VersionSummary {
    #[schema(value_type = String)]
    workflow_id: WorkflowId,
    number: i32,
    state: StateName,
    /// Whether this is the Workflow's Active Version.
    active: bool,
    published_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct VersionList {
    /// Oldest first.
    versions: Vec<VersionSummary>,
}

/// A Version with its Definition.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct VersionInfo {
    #[serde(flatten)]
    summary: VersionSummary,
    /// The Definition as stored, in its canonical JSON form.
    #[schema(value_type = Object)]
    definition: Value,
}

/// A Version just saved or published, with what validation found.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SavedVersion {
    version: VersionInfo,
    /// Every Diagnostic for the Definition. A Draft may be saved with errors; publishing
    /// requires none.
    #[schema(value_type = Vec<DiagnosticSchema>)]
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ValidationReport {
    /// Whether the Definition has no error Diagnostics, so it can be published (for the
    /// `publish` target) or is complete enough to publish later (for `draft`).
    valid: bool,
    #[schema(value_type = Vec<DiagnosticSchema>)]
    diagnostics: Vec<Diagnostic>,
}

/// What a Definition is validated for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TargetName {
    /// Problems only publishing needs solved, such as unknown Queues or Workflows and
    /// disabled Actions, are warnings.
    #[default]
    Draft,
    /// Those problems are errors, as they are when publishing.
    Publish,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidateQuery {
    /// What to validate for. Defaults to `draft`.
    #[serde(default)]
    target: TargetName,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiffQuery {
    /// The Version number to diff from.
    from: i32,
    /// The Version number to diff to.
    to: i32,
}

/// What changed between two Versions of one Workflow.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct VersionDiff {
    from: i32,
    to: i32,
    /// An RFC 6902 JSON Patch that turns `from`'s Definition into `to`'s, one operation per
    /// changed field; empty when the Definitions are equal.
    #[schema(value_type = Vec<Object>)]
    patch: json_patch::Patch,
}

fn workflows() -> PgWorkflowRepository {
    PgWorkflowRepository
}

fn versions() -> PgVersionRepository {
    PgVersionRepository
}

/// Every Workflow with its Active Version.
#[utoipa::path(
    get,
    path = "/v1/workflows",
    tag = "workflows",
    security(("token" = [])),
    responses((status = OK, body = WorkflowList), AuthErrors)
)]
pub(crate) async fn list_workflows(
    _: Agent,
    State(state): State<ApiState>,
) -> Result<Json<WorkflowList>, ApiError> {
    let mut conn = acquire(&state).await?;
    let list = workflows().list(&mut conn).await.map_err(store_error)?;
    Ok(Json(WorkflowList {
        workflows: list.into_iter().map(WorkflowInfo::from).collect(),
    }))
}

/// One Workflow.
#[utoipa::path(
    get,
    path = "/v1/workflows/{workflow_id}",
    tag = "workflows",
    security(("token" = [])),
    params(("workflow_id" = String, Path, description = "The Workflow's id.")),
    responses(
        (status = OK, body = WorkflowInfo),
        (status = NOT_FOUND, description = "`workflow_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn get_workflow(
    _: Agent,
    State(state): State<ApiState>,
    Path(workflow_id): Path<WorkflowId>,
) -> Result<Json<WorkflowInfo>, ApiError> {
    let mut conn = acquire(&state).await?;
    let workflow = workflows()
        .get(&mut conn, &workflow_id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| store_error(StoreError::WorkflowNotFound(workflow_id)))?;
    Ok(Json(workflow.into()))
}

/// The Workflow's Versions, oldest first, without their Definitions.
#[utoipa::path(
    get,
    path = "/v1/workflows/{workflow_id}/versions",
    tag = "workflows",
    security(("token" = [])),
    params(("workflow_id" = String, Path, description = "The Workflow's id.")),
    responses(
        (status = OK, body = VersionList),
        (status = NOT_FOUND, description = "`workflow_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn list_versions(
    _: Agent,
    State(state): State<ApiState>,
    Path(workflow_id): Path<WorkflowId>,
) -> Result<Json<VersionList>, ApiError> {
    let mut conn = acquire(&state).await?;
    let workflow = existing_workflow(&mut conn, &workflow_id).await?;
    let list = versions()
        .list(&mut conn, &workflow_id)
        .await
        .map_err(store_error)?;
    Ok(Json(VersionList {
        versions: list
            .into_iter()
            .map(|v| summary(v, workflow.active_version))
            .collect(),
    }))
}

/// Saves the request body, a Definition, as a new Draft of the Workflow, numbered after its
/// latest Version. The Workflow is created by its first Draft. Any number of Drafts may exist
/// at once. Semantic errors do not stop a Draft from being saved: they come back in
/// `diagnostics`. A body that does not parse as a Definition is refused with
/// `invalid_definition`, and one whose `id` is not the Workflow's with `workflow_id_mismatch`.
#[utoipa::path(
    post,
    path = "/v1/workflows/{workflow_id}/versions",
    tag = "workflows",
    security(("token" = [])),
    params(("workflow_id" = String, Path, description = "The Workflow's id.")),
    request_body(content = Object, description = "A Definition (see `GET /v1/schema`)."),
    responses(
        (status = CREATED, body = SavedVersion),
        (status = BAD_REQUEST, description = "`malformed_request`: not JSON.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_definition` or `workflow_id_mismatch`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn create_draft(
    _: Agent,
    State(state): State<ApiState>,
    Path(workflow_id): Path<WorkflowId>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<SavedVersion>), ApiError> {
    let mut conn = acquire(&state).await?;
    let (definition, diagnostics) = parse(&state, &mut conn, &workflow_id, body).await?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    match workflows().create(&mut tx, &workflow_id).await {
        Ok(_) | Err(StoreError::WorkflowExists(_)) => {}
        Err(error) => return Err(store_error(error)),
    }
    let version = versions()
        .create_draft(&mut tx, &definition)
        .await
        .map_err(store_error)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    tracing::info!(workflow = %workflow_id, version = version.number.get(), "draft created");
    Ok((StatusCode::CREATED, Json(saved(version, None, diagnostics))))
}

/// One Version with its Definition.
#[utoipa::path(
    get,
    path = "/v1/workflows/{workflow_id}/versions/{number}",
    tag = "workflows",
    security(("token" = [])),
    params(
        ("workflow_id" = String, Path, description = "The Workflow's id."),
        ("number" = i32, Path, description = "The Version's number."),
    ),
    responses(
        (status = OK, body = VersionInfo),
        (status = NOT_FOUND, description = "`workflow_not_found` or `version_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn get_version(
    _: Agent,
    State(state): State<ApiState>,
    Path((workflow_id, number)): Path<(WorkflowId, i32)>,
) -> Result<Json<VersionInfo>, ApiError> {
    let number = version_number(number)?;
    let mut conn = acquire(&state).await?;
    let workflow = existing_workflow(&mut conn, &workflow_id).await?;
    let version = existing_version(&mut conn, &workflow_id, number).await?;
    Ok(Json(info(version, workflow.active_version)))
}

/// Replaces a Draft's Definition in place, keeping its number. Refused with
/// `version_published` once the Version is Published. The body is treated as in
/// `create_draft`.
#[utoipa::path(
    put,
    path = "/v1/workflows/{workflow_id}/versions/{number}",
    tag = "workflows",
    security(("token" = [])),
    params(
        ("workflow_id" = String, Path, description = "The Workflow's id."),
        ("number" = i32, Path, description = "The Draft's number."),
    ),
    request_body(content = Object, description = "A Definition (see `GET /v1/schema`)."),
    responses(
        (status = OK, body = SavedVersion),
        (status = BAD_REQUEST, description = "`malformed_request`: not JSON.", body = ErrorBody),
        (status = NOT_FOUND, description = "`workflow_not_found` or `version_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`version_published`.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_definition` or `workflow_id_mismatch`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn replace_draft(
    _: Agent,
    State(state): State<ApiState>,
    Path((workflow_id, number)): Path<(WorkflowId, i32)>,
    Json(body): Json<Value>,
) -> Result<Json<SavedVersion>, ApiError> {
    let number = version_number(number)?;
    let mut conn = acquire(&state).await?;
    let (definition, diagnostics) = parse(&state, &mut conn, &workflow_id, body).await?;
    let version = versions()
        .replace_draft(&mut conn, &workflow_id, number, &definition)
        .await
        .map_err(store_error)?;
    tracing::info!(workflow = %workflow_id, version = number.get(), "draft replaced");
    Ok(Json(saved(version, None, diagnostics)))
}

/// Validates a stored Version's Definition for the `draft` (default) or `publish` target.
/// Diagnostics are the result, not an error: the response is 200 even when they hold errors.
#[utoipa::path(
    post,
    path = "/v1/workflows/{workflow_id}/versions/{number}/validate",
    tag = "workflows",
    security(("token" = [])),
    params(
        ("workflow_id" = String, Path, description = "The Workflow's id."),
        ("number" = i32, Path, description = "The Version's number."),
        ValidateQuery,
    ),
    responses(
        (status = OK, body = ValidationReport),
        (status = NOT_FOUND, description = "`workflow_not_found` or `version_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn validate_version(
    _: Agent,
    State(state): State<ApiState>,
    Path((workflow_id, number)): Path<(WorkflowId, i32)>,
    Query(query): Query<ValidateQuery>,
) -> Result<Json<ValidationReport>, ApiError> {
    let number = version_number(number)?;
    let mut conn = acquire(&state).await?;
    let version = existing_version(&mut conn, &workflow_id, number).await?;
    let resources = known_resources(&state, &mut conn).await?;
    let diagnostics =
        validator(&state, target(query.target), &resources).validate(&version.definition);
    Ok(Json(report(diagnostics)))
}

/// Validates the request body, a Definition that need not be stored, for the `draft`
/// (default) or `publish` target. Diagnostics are the result, so a Definition with errors
/// still gets 200; a body that is not JSON gets `malformed_request`, and one that does not
/// parse as a Definition comes back as a single Diagnostic.
#[utoipa::path(
    post,
    path = "/v1/validate",
    tag = "workflows",
    security(("token" = [])),
    params(ValidateQuery),
    request_body(content = Object, description = "A Definition (see `GET /v1/schema`)."),
    responses(
        (status = OK, body = ValidationReport),
        (status = BAD_REQUEST, description = "`malformed_request`: not JSON.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn validate_definition(
    _: Agent,
    State(state): State<ApiState>,
    Query(query): Query<ValidateQuery>,
    Json(body): Json<Value>,
) -> Result<Json<ValidationReport>, ApiError> {
    let mut conn = acquire(&state).await?;
    let resources = known_resources(&state, &mut conn).await?;
    let found = validator(&state, target(query.target), &resources).validate_value(body);
    Ok(Json(report(found.diagnostics)))
}

/// Publishes a Draft after validating it for publishing: unknown Queues and Workflows and
/// disabled or dangerous Actions are errors, and any error refuses with `invalid_definition`
/// and the Diagnostics. A Published Version never changes again. Needs an `admin` Token, which
/// is also what keeps Versions that use dangerous Actions in human hands.
#[utoipa::path(
    post,
    path = "/v1/workflows/{workflow_id}/versions/{number}/publish",
    tag = "workflows",
    security(("token" = [])),
    params(
        ("workflow_id" = String, Path, description = "The Workflow's id."),
        ("number" = i32, Path, description = "The Draft's number."),
    ),
    responses(
        (status = OK, body = SavedVersion),
        (status = NOT_FOUND, description = "`workflow_not_found` or `version_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`version_published`, or `version_changed` when the Draft was replaced meanwhile.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_definition`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn publish_version(
    _: Admin,
    State(state): State<ApiState>,
    Path((workflow_id, number)): Path<(WorkflowId, i32)>,
) -> Result<Json<SavedVersion>, ApiError> {
    let number = version_number(number)?;
    let mut conn = acquire(&state).await?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let draft = existing_version(&mut tx, &workflow_id, number).await?;
    if draft.state == VersionState::Published {
        return Err(store_error(StoreError::VersionPublished {
            workflow: workflow_id,
            number,
        }));
    }
    let resources = known_resources(&state, &mut tx).await?;
    let diagnostics = validator(&state, Target::Publish, &resources).validate(&draft.definition);
    if diagnostics.iter().any(Diagnostic::is_error) {
        return Err(ApiError::invalid_definition(diagnostics));
    }
    let published = versions()
        .publish(&mut tx, &workflow_id, number, state.clock.now())
        .await
        .map_err(store_error)?;
    // The Draft may have been replaced after it was validated; publishing what nobody
    // validated would defeat the check.
    if published.definition != draft.definition {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::VersionChanged,
            "the Draft was replaced while it was being published; publish it again",
        ));
    }
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    tracing::info!(workflow = %workflow_id, version = number.get(), "version published");
    let workflow = existing_workflow(&mut conn, &workflow_id).await?;
    Ok(Json(saved(published, workflow.active_version, diagnostics)))
}

/// Makes a Published Version the Workflow's Active Version: new Runs start on it and the
/// scheduler follows its cron Triggers. Activating an older Version is a rollback; Runs
/// already started keep their Version (ADR-0009). Needs an `admin` Token.
#[utoipa::path(
    post,
    path = "/v1/workflows/{workflow_id}/versions/{number}/activate",
    tag = "workflows",
    security(("token" = [])),
    params(
        ("workflow_id" = String, Path, description = "The Workflow's id."),
        ("number" = i32, Path, description = "The Published Version's number."),
    ),
    responses(
        (status = OK, body = WorkflowInfo),
        (status = NOT_FOUND, description = "`workflow_not_found` or `version_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`version_not_published`: the Version is a Draft.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn activate_version(
    _: Admin,
    State(state): State<ApiState>,
    Path((workflow_id, number)): Path<(WorkflowId, i32)>,
) -> Result<Json<WorkflowInfo>, ApiError> {
    let number = version_number(number)?;
    let mut conn = acquire(&state).await?;
    workflows()
        .set_active_version(&mut conn, &workflow_id, number)
        .await
        .map_err(store_error)?;
    tracing::info!(workflow = %workflow_id, version = number.get(), "version activated");
    let workflow = existing_workflow(&mut conn, &workflow_id).await?;
    Ok(Json(workflow.into()))
}

/// What changed between two Versions of a Workflow, as an RFC 6902 JSON Patch over their
/// Definitions in canonical form. Works on Drafts and Published Versions alike.
#[utoipa::path(
    get,
    path = "/v1/workflows/{workflow_id}/diff",
    tag = "workflows",
    security(("token" = [])),
    params(("workflow_id" = String, Path, description = "The Workflow's id."), DiffQuery),
    responses(
        (status = OK, body = VersionDiff),
        (status = NOT_FOUND, description = "`workflow_not_found` or `version_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn diff_versions(
    _: Agent,
    State(state): State<ApiState>,
    Path(workflow_id): Path<WorkflowId>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<VersionDiff>, ApiError> {
    let (from, to) = (version_number(query.from)?, version_number(query.to)?);
    let mut conn = acquire(&state).await?;
    existing_workflow(&mut conn, &workflow_id).await?;
    let from_version = existing_version(&mut conn, &workflow_id, from).await?;
    let to_version = existing_version(&mut conn, &workflow_id, to).await?;
    let patch = json_patch::diff(
        &definition_value(&from_version.definition),
        &definition_value(&to_version.definition),
    );
    Ok(Json(VersionDiff {
        from: from.get(),
        to: to.get(),
        patch,
    }))
}

pub(super) async fn acquire(
    state: &ApiState,
) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, ApiError> {
    state
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))
}

/// Parses `body` as a Definition of `workflow_id` and validates it as a Draft. Only a body
/// that is not a Definition at all, or is another Workflow's, is refused.
async fn parse(
    state: &ApiState,
    conn: &mut PgConnection,
    workflow_id: &WorkflowId,
    body: Value,
) -> Result<(Definition, Vec<Diagnostic>), ApiError> {
    let resources = known_resources(state, conn).await?;
    let found = validator(state, Target::Draft, &resources).validate_value(body);
    let Some(definition) = found.definition else {
        return Err(ApiError::invalid_definition(found.diagnostics));
    };
    if definition.id != *workflow_id {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::WorkflowIdMismatch,
            format!(
                "the Definition's `id` is `{}` but the request is for Workflow `{workflow_id}`",
                definition.id
            ),
        ));
    }
    Ok((definition, found.diagnostics))
}

pub(super) fn validator<'a>(
    state: &'a ApiState,
    target: Target,
    resources: &'a KnownResources,
) -> Validator<'a> {
    Validator::new(target, state.actions.as_ref(), resources)
}

pub(super) async fn known_resources(
    state: &ApiState,
    conn: &mut PgConnection,
) -> Result<KnownResources, ApiError> {
    let mut known = KnownResources::new();
    known.queues = state.queues.names().await;
    known.workflows = workflows()
        .list(conn)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(|w| w.id.as_str().to_owned())
        .collect();
    Ok(known)
}

fn target(name: TargetName) -> Target {
    match name {
        TargetName::Draft => Target::Draft,
        TargetName::Publish => Target::Publish,
    }
}

fn report(diagnostics: Vec<Diagnostic>) -> ValidationReport {
    ValidationReport {
        valid: !diagnostics.iter().any(Diagnostic::is_error),
        diagnostics,
    }
}

pub(super) fn version_number(number: i32) -> Result<VersionNumber, ApiError> {
    VersionNumber::new(number)
        .ok_or_else(|| ApiError::malformed_request("a Version number is a positive integer"))
}

async fn existing_workflow(conn: &mut PgConnection, id: &WorkflowId) -> Result<Workflow, ApiError> {
    workflows()
        .get(conn, id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| store_error(StoreError::WorkflowNotFound(id.clone())))
}

pub(super) async fn existing_version(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
) -> Result<Version, ApiError> {
    if let Some(version) = versions()
        .get(conn, workflow, number)
        .await
        .map_err(store_error)?
    {
        return Ok(version);
    }
    existing_workflow(conn, workflow).await?;
    Err(store_error(StoreError::VersionNotFound {
        workflow: workflow.clone(),
        number,
    }))
}

fn definition_value(definition: &Definition) -> Value {
    serde_json::to_value(definition)
        .unwrap_or_else(|_| unreachable!("a Definition serialises to JSON"))
}

fn summary(version: Version, active: Option<VersionNumber>) -> VersionSummary {
    VersionSummary {
        active: active == Some(version.number),
        workflow_id: version.workflow_id,
        number: version.number.get(),
        state: match version.state {
            VersionState::Draft => StateName::Draft,
            VersionState::Published => StateName::Published,
        },
        published_at: version.published_at,
        created_at: version.created_at,
    }
}

fn info(version: Version, active: Option<VersionNumber>) -> VersionInfo {
    let definition = definition_value(&version.definition);
    VersionInfo {
        summary: summary(version, active),
        definition,
    }
}

fn saved(
    version: Version,
    active: Option<VersionNumber>,
    diagnostics: Vec<Diagnostic>,
) -> SavedVersion {
    SavedVersion {
        version: info(version, active),
        diagnostics,
    }
}

impl From<Workflow> for WorkflowInfo {
    fn from(workflow: Workflow) -> Self {
        Self {
            id: workflow.id,
            active_version: workflow.active_version.map(VersionNumber::get),
            created_at: workflow.created_at,
        }
    }
}

pub(super) fn store_error(error: StoreError) -> ApiError {
    let (status, code) = match &error {
        StoreError::WorkflowNotFound(_) => (StatusCode::NOT_FOUND, ErrorCode::WorkflowNotFound),
        StoreError::VersionNotFound { .. } => (StatusCode::NOT_FOUND, ErrorCode::VersionNotFound),
        StoreError::VersionPublished { .. } => (StatusCode::CONFLICT, ErrorCode::VersionPublished),
        StoreError::VersionNotPublished { .. } => {
            (StatusCode::CONFLICT, ErrorCode::VersionNotPublished)
        }
        StoreError::DefinitionMismatch { .. } => (
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::WorkflowIdMismatch,
        ),
        _ => return ApiError::internal(&error),
    };
    // These variants name only ids the caller sent.
    ApiError::new(status, code, error.to_string())
}
