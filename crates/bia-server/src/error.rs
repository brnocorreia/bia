//! The error body every endpoint fails with.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bia_core::validation::Diagnostic;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A stable identifier for what went wrong with a request. Agents match on it, so codes may be
/// added, but renaming or removing one needs an ADR (ADR-0042).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// No route has this path.
    RouteNotFound,
    /// The route exists but does not take this HTTP method.
    MethodNotAllowed,
    /// The request body or parameters are not what the route takes: not JSON, the wrong
    /// `Content-Type`, or the wrong shape.
    MalformedRequest,
    /// The request body is larger than the server accepts.
    RequestTooLarge,
    /// The Definition has error Diagnostics; they are listed in `diagnostics`.
    InvalidDefinition,
    /// The route needs a Token and the request carries none.
    MissingToken,
    /// The `Authorization` header is not `Bearer <token>`, or the Token is unknown or revoked.
    InvalidToken,
    /// The Token's Scope does not allow this route.
    InsufficientScope,
    /// No Token has this id.
    TokenNotFound,
    /// Bootstrapping is off because the server has no bootstrap key configured.
    BootstrapDisabled,
    /// An unrevoked admin Token exists, so bootstrapping refuses to mint another; create
    /// Tokens with it instead.
    AlreadyBootstrapped,
    /// No Workflow has this id.
    WorkflowNotFound,
    /// The Workflow has no Version with this number.
    VersionNotFound,
    /// The Version is Published, so it cannot be replaced or published again.
    VersionPublished,
    /// The Version is a Draft, so it cannot be activated.
    VersionNotPublished,
    /// The Draft was replaced while it was being published; publish again to publish the new
    /// content.
    VersionChanged,
    /// The Definition's `id` is not the Workflow's id in the path.
    WorkflowIdMismatch,
    /// The Workflow has no Active Version, so no Run can start on it.
    NoActiveVersion,
    /// The Workflow's Active Version has no manual Trigger (with the id the request named, if
    /// it named one).
    NoManualTrigger,
    /// The Run input does not match the Active Version's `input_schema`; no Run was created.
    InvalidRunInput,
    /// A Dry Run Mock is not in the documented format, or names a Step that cannot be Mocked.
    InvalidMock,
    /// The Dry Run took more transitions than the server allows and was stopped.
    DryRunTooLong,
    /// No Run has this id.
    RunNotFound,
    /// The Run already finished, so it cannot be cancelled.
    RunFinished,
    /// The Run has not finished, so it cannot be retried.
    RunNotFinished,
    /// The Run cannot be resumed: it is not `failed`, or it failed before any Step ran.
    RunNotResumable,
    /// No Secret has this name.
    SecretNotFound,
    /// The server has no `BIA_MASTER_KEY`, so it cannot store Secrets.
    MasterKeyMissing,
    /// The server cannot serve requests yet: the database is unreachable or migrations are
    /// pending.
    NotReady,
    /// The server failed; the request may succeed if retried.
    InternalError,
}

/// The body of every error response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    pub code: ErrorCode,
    /// Human-readable explanation. Its wording may change; match on `code`.
    pub message: String,
    /// The Diagnostics behind the error, in document order. Absent when none apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<DiagnosticSchema>)]
    pub diagnostics: Vec<Diagnostic>,
}

// Documents `bia_core::validation::Diagnostic`, which has no `ToSchema` because `bia-core`
// stays free of API concerns; a test checks the two list the same fields.

/// One validation finding about a Definition.
#[derive(ToSchema)]
#[schema(as = Diagnostic)]
#[allow(dead_code)]
pub(crate) struct DiagnosticSchema {
    /// JSON Pointer to the offending value, e.g. `/steps/0/params/url`.
    path: String,
    /// A stable snake_case phrase naming the problem, listed in `docs/dsl.md`.
    code: String,
    severity: SeveritySchema,
    /// Human-readable explanation. Its wording may change; match on `code`.
    message: String,
    /// A likely fix, such as the nearest known name.
    suggestion: Option<String>,
}

/// Only errors block publishing.
#[derive(ToSchema)]
#[schema(as = Severity)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum SeveritySchema {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    status: StatusCode,
    body: ErrorBody,
}

impl ApiError {
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorBody {
                code,
                message: message.into(),
                diagnostics: Vec::new(),
            },
        }
    }

    pub fn with_diagnostics(mut self, diagnostics: Vec<Diagnostic>) -> Self {
        self.body.diagnostics = diagnostics;
        self
    }

    /// 422 carrying every Diagnostic, warnings included, so a caller fixes them in one pass.
    pub fn invalid_definition(diagnostics: Vec<Diagnostic>) -> Self {
        let errors = diagnostics.iter().filter(|d| d.is_error()).count();
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::InvalidDefinition,
            format!("the Definition has {errors} error(s); see `diagnostics`"),
        )
        .with_diagnostics(diagnostics)
    }

    pub fn malformed_request(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::MalformedRequest,
            message,
        )
    }

    /// 500 with a fixed message: `error` is logged, never returned, since it may carry
    /// details a caller should not see.
    pub fn internal(error: &dyn std::error::Error) -> Self {
        tracing::error!(%error, "request failed");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::InternalError,
            "the server failed; see its logs",
        )
    }

    pub(crate) fn route_not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            ErrorCode::RouteNotFound,
            "no route has this path; see /openapi.json",
        )
    }

    pub(crate) fn method_not_allowed() -> Self {
        Self::new(
            StatusCode::METHOD_NOT_ALLOWED,
            ErrorCode::MethodNotAllowed,
            "this route does not take this method; see /openapi.json",
        )
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn body(&self) -> &ErrorBody {
        &self.body
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, axum::Json(self.body)).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}
