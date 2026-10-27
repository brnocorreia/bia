//! Dry Run: a Version or an ad-hoc Definition run in memory with Mocks for its side effects,
//! answered with a Timeline shaped like a real Run's (ADR-0046). Nothing is stored. `agent`
//! Tokens may dry-run (ADR-0008).

use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::StatusCode;
use bia_core::action::ActionError;
use bia_core::dsl::WorkflowId;
use bia_core::validation::{Diagnostic, Target};
use bia_engine::{DryRun, EngineError, Interpreter, Mock, MockOutcome, Mocks};
use bia_store::run::{RunId, RunStatus};
use bia_store::timeline::{NewTimelineEntry, TimelineEntry};
use bia_store::version::VersionNumber;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

use super::run::{RunStatusName, TimelineEntryInfo, entry};
use super::workflow::{acquire, existing_version, known_resources, validator, version_number};
use crate::auth::{Agent, AuthErrors};
use crate::error::DiagnosticSchema;
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path};

/// What a Dry Run of a stored Version takes.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DryRunRequest {
    /// The Run input. Defaults to `{}`. Checked against the Definition's `input_schema`, as a
    /// real Run's is.
    #[serde(default = "empty_input")]
    input: Value,
    /// Mocks by Step id. See [`MockSchema`].
    #[serde(default)]
    #[schema(value_type = BTreeMap<String, MockSchema>)]
    mocks: BTreeMap<String, Value>,
}

/// What a Dry Run of a Definition that need not be stored takes.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DefinitionDryRunRequest {
    /// A Definition (see `GET /v1/schema`). Its Workflow need not exist.
    #[schema(value_type = Object)]
    definition: Value,
    /// The Run input. Defaults to `{}`.
    #[serde(default = "empty_input")]
    input: Value,
    /// Mocks by Step id. See [`MockSchema`].
    #[serde(default)]
    #[schema(value_type = BTreeMap<String, MockSchema>)]
    mocks: BTreeMap<String, Value>,
}

fn empty_input() -> Value {
    json!({})
}

/// What a Step returns in a Dry Run instead of calling its Action. Give exactly one of
/// `output`, `error` or `attempts`.
///
/// Without a Mock, a `pure` Action runs for real and a `side_effecting` one returns the
/// Output of its first example, or, having no example, fails with the `no_mock` code. A Mock
/// also applies to a `pure` Action, which lets a Dry Run force it to fail. Only Steps that
/// call an Action can be Mocked: not `flow.*` Steps or Blocks.
#[derive(Debug, ToSchema)]
#[allow(dead_code)]
pub(crate) struct MockSchema {
    /// The Output every Attempt returns. Any JSON value, `null` included.
    output: Option<Value>,
    /// The error every Attempt fails with: `{ "class": "retryable" | "fatal", "message",
    /// "details"? }`. A `retryable` error follows the Step's retry policy exactly as a real one
    /// does.
    error: Option<Value>,
    /// One `{ "output" }` or `{ "error" }` per Attempt: the first Attempt gets the first, and
    /// the last repeats for the Attempts after it. Use it to fail twice and then succeed.
    attempts: Option<Vec<Value>>,
}

/// A Dry Run's result.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DryRunReport {
    /// Always `true`; marks the Timeline as not a real Run's.
    dry_run: bool,
    /// How the Run ended: `succeeded` or `failed`.
    status: RunStatusName,
    input: Value,
    /// The entries a real Run's Timeline would hold, in the same shape and order. Waits take
    /// no real time, but their entries show the time they would have taken.
    timeline: Vec<TimelineEntryInfo>,
    /// Validation findings that are not errors, such as problems only publishing needs solved.
    #[schema(value_type = Vec<DiagnosticSchema>)]
    diagnostics: Vec<Diagnostic>,
}

/// Dry-runs a stored Version, a Draft or a Published one: pure Actions run for real, side
/// effects return Mocks, and waits resolve instantly. Nothing is stored and nothing outside
/// the server is touched. The Definition must have no error Diagnostics (`invalid_definition`
/// otherwise); a Mock that names a missing or unMockable Step, or is not in the documented
/// format, is refused with `invalid_mock`.
#[utoipa::path(
    post,
    path = "/v1/workflows/{workflow_id}/versions/{number}/dry-run",
    tag = "dry-run",
    security(("token" = [])),
    params(
        ("workflow_id" = String, Path, description = "The Workflow's id."),
        ("number" = i32, Path, description = "The Version's number."),
    ),
    request_body(content = DryRunRequest, description = "The input and Mocks; send `{}` for none."),
    responses(
        (status = OK, body = DryRunReport),
        (status = BAD_REQUEST, description = "`malformed_request`.", body = ErrorBody),
        (status = NOT_FOUND, description = "`workflow_not_found` or `version_not_found`.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_definition`, `invalid_mock` or `dry_run_too_long`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn dry_run_version(
    _: Agent,
    State(state): State<ApiState>,
    Path((workflow_id, number)): Path<(WorkflowId, i32)>,
    Json(request): Json<DryRunRequest>,
) -> Result<Json<DryRunReport>, ApiError> {
    let number = version_number(number)?;
    let mut conn = acquire(&state).await?;
    let version = existing_version(&mut conn, &workflow_id, number).await?;
    let resources = known_resources(&mut conn).await?;
    let diagnostics = validator(&state, Target::Draft, &resources).validate(&version.definition);
    drop(conn);
    run(
        &state,
        version.definition,
        version.number,
        diagnostics,
        request.input,
        request.mocks,
    )
    .await
}

/// [`dry_run_version`] for a Definition in the request body, which need not be stored.
#[utoipa::path(
    post,
    path = "/v1/dry-run",
    tag = "dry-run",
    security(("token" = [])),
    request_body = DefinitionDryRunRequest,
    responses(
        (status = OK, body = DryRunReport),
        (status = BAD_REQUEST, description = "`malformed_request`.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_definition`, `invalid_mock` or `dry_run_too_long`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn dry_run_definition(
    _: Agent,
    State(state): State<ApiState>,
    Json(request): Json<DefinitionDryRunRequest>,
) -> Result<Json<DryRunReport>, ApiError> {
    let mut conn = acquire(&state).await?;
    let resources = known_resources(&mut conn).await?;
    let found = validator(&state, Target::Draft, &resources).validate_value(request.definition);
    drop(conn);
    let Some(definition) = found.definition else {
        return Err(ApiError::invalid_definition(found.diagnostics));
    };
    let version = VersionNumber::new(1).unwrap_or_else(|| unreachable!("1 is a Version number"));
    run(
        &state,
        definition,
        version,
        found.diagnostics,
        request.input,
        request.mocks,
    )
    .await
}

async fn run(
    state: &ApiState,
    definition: bia_core::dsl::Definition,
    version: VersionNumber,
    diagnostics: Vec<Diagnostic>,
    input: Value,
    mocks: BTreeMap<String, Value>,
) -> Result<Json<DryRunReport>, ApiError> {
    if diagnostics.iter().any(Diagnostic::is_error) {
        return Err(ApiError::invalid_definition(diagnostics));
    }
    let mocks = parse_mocks(mocks)?;
    let interpreter = Interpreter::new(
        state.pool.clone(),
        state.actions.clone(),
        state.clock.clone(),
    );
    let result = interpreter
        .dry_run(DryRun {
            definition,
            version,
            input: input.clone(),
            mocks,
        })
        .await
        .map_err(engine_error)?;
    let timeline = result
        .timeline
        .into_iter()
        .zip(1..)
        .map(|(new, seq)| entry(recorded(new, seq)))
        .collect();
    Ok(Json(DryRunReport {
        dry_run: true,
        status: match result.status {
            RunStatus::Succeeded => RunStatusName::Succeeded,
            _ => RunStatusName::Failed,
        },
        input,
        timeline,
        diagnostics,
    }))
}

/// The entry as the store would hand it back, so a Dry Run's entries are rendered by the very
/// code that renders a real Run's.
fn recorded(new: NewTimelineEntry, seq: i32) -> TimelineEntry {
    TimelineEntry {
        run_id: RunId::from_uuid(uuid::Uuid::nil()),
        seq,
        step: new.step,
        status: new.status,
        input: new.input,
        output: new.output,
        error: new.error,
        started_at: new.started_at,
        finished_at: new.finished_at,
    }
}

fn parse_mocks(raw: BTreeMap<String, Value>) -> Result<Mocks, ApiError> {
    raw.into_iter()
        .map(|(step, value)| match parse_mock(&value) {
            Ok(mock) => Ok((step, mock)),
            Err(reason) => Err(invalid_mock(format!("the Mock for Step `{step}` {reason}"))),
        })
        .collect()
}

fn parse_mock(value: &Value) -> Result<Mock, String> {
    let object = value
        .as_object()
        .ok_or("must be an object with `output`, `error` or `attempts`")?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !["output", "error", "attempts"].contains(&key.as_str()))
    {
        return Err(format!("has an unknown field `{unknown}`"));
    }
    if object.len() != 1 {
        return Err("must give exactly one of `output`, `error` or `attempts`".to_owned());
    }
    let attempts = match object.get("attempts") {
        Some(Value::Array(attempts)) => {
            attempts.iter().map(parse_outcome).collect::<Result<_, _>>()
        }
        Some(_) => Err("`attempts` must be a list".to_owned()),
        None => parse_outcome(value).map(|outcome| vec![outcome]),
    }?;
    Mock::new(attempts).ok_or_else(|| "`attempts` must not be empty".to_owned())
}

fn parse_outcome(value: &Value) -> Result<MockOutcome, String> {
    let object = value
        .as_object()
        .filter(|object| object.len() == 1)
        .ok_or("must give exactly one of `output` or `error` per Attempt")?;
    if let Some(output) = object.get("output") {
        return Ok(MockOutcome::Output(output.clone()));
    }
    match object.get("error") {
        Some(error) => serde_json::from_value::<ActionError>(error.clone())
            .map(MockOutcome::Error)
            .map_err(|e| {
                format!("has an `error` that is not `{{ class, message, details? }}`: {e}")
            }),
        None => Err("must give exactly one of `output` or `error` per Attempt".to_owned()),
    }
}

fn invalid_mock(message: String) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::InvalidMock,
        message,
    )
}

fn engine_error(error: EngineError) -> ApiError {
    match error {
        // These name only Step ids the caller sent or the Definition holds.
        EngineError::UnknownMockStep { .. } | EngineError::UnmockableStep { .. } => {
            invalid_mock(error.to_string())
        }
        EngineError::DryRunTooLong { .. } => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::DryRunTooLong,
            error.to_string(),
        ),
        other => ApiError::internal(&other),
    }
}
