//! How a failed Attempt is recorded on the Timeline (ADR-0033).

use bia_core::action::ActionError;
use serde::Serialize;
use serde_json::Value;

/// Why an Attempt failed: the Action's own error, or one the interpreter raised before or
/// around the call, which also carries a [`FailureCode`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Failure {
    #[serde(flatten)]
    pub(crate) error: ActionError,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) code: Option<FailureCode>,
}

impl Failure {
    pub(crate) fn from_action(error: ActionError) -> Self {
        Self { error, code: None }
    }

    pub(crate) fn raised(code: FailureCode, error: ActionError) -> Self {
        Self {
            error,
            code: Some(code),
        }
    }

    pub(crate) fn is_retryable(&self) -> bool {
        self.error.is_retryable()
    }

    /// The `error` value of the Timeline entry.
    pub(crate) fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Stable identifiers for failures the interpreter raises itself, which agents match on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureCode {
    /// The Run input does not match the Definition's `input_schema`.
    InvalidInput,
    /// The Attempt ran past the Step's `timeout` and was cancelled.
    Timeout,
    /// The Step's Action is not in the Action Registry.
    UnknownAction,
    /// The Step's Action is disabled on this server (ADR-0014).
    DisabledAction,
    /// A Template in Params did not evaluate.
    InvalidTemplate,
    /// The resolved Params break the Action's param schema.
    InvalidParams,
    /// Params read a Secret no Secret Provider knows.
    UnknownSecret,
    /// The Step uses something this build cannot run yet, such as a Block.
    UnsupportedStep,
}
