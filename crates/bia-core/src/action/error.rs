//! The classified error an Action returns (ADR-0012).

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Why an Attempt failed, and whether trying again could help.
///
/// The message and details reach the Timeline, so they must never contain a resolved Secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{class} error: {message}")]
pub struct ActionError {
    class: ErrorClass,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl ActionError {
    /// A failure that may pass on its own: network errors, 5xx responses, timeouts.
    pub fn retryable(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Retryable, message)
    }

    /// A failure that will repeat on every Attempt: invalid Params, 4xx responses.
    pub fn fatal(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Fatal, message)
    }

    pub fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn class(&self) -> ErrorClass {
        self.class
    }

    pub fn is_retryable(&self) -> bool {
        self.class == ErrorClass::Retryable
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn details(&self) -> Option<&Value> {
        self.details.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Retryable,
    Fatal,
}

impl fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Retryable => "retryable",
            Self::Fatal => "fatal",
        })
    }
}
