//! The schema layer: parse errors as Diagnostics.

use super::suggest::did_you_mean;
use super::{Code, Diagnostic, Severity};
use crate::dsl::{ParseError, ParseErrorKind};

impl From<ParseError> for Diagnostic {
    fn from(error: ParseError) -> Self {
        let (code, suggestion) = match &error.kind {
            ParseErrorKind::Syntax { .. } => (Code::InvalidSyntax, None),
            ParseErrorKind::MissingSchemaVersion => (Code::MissingSchemaVersion, None),
            ParseErrorKind::UnsupportedSchemaVersion { .. } => {
                (Code::UnsupportedSchemaVersion, None)
            }
            ParseErrorKind::UnknownKind { .. } => (Code::UnknownKind, None),
            ParseErrorKind::Shape { message } => classify(message),
        };
        Self {
            path: error.path,
            code,
            severity: Severity::Error,
            message: error.kind.to_string(),
            suggestion,
        }
    }
}

/// Picks a code for a shape error from serde's message, which is the only place the kind of
/// problem is recorded. Typos in field names and enum values get a suggestion from the
/// alternatives serde lists.
fn classify(message: &str) -> (Code, Option<String>) {
    if let Some(rest) = message.strip_prefix("unknown field ") {
        return (Code::UnknownField, suggest_from(rest));
    }
    if let Some(rest) = message.strip_prefix("unknown variant ") {
        return (Code::InvalidValue, suggest_from(rest));
    }
    if message.starts_with("missing field ") {
        return (Code::MissingField, None);
    }
    if message.contains(" is not allowed on a ") {
        return (Code::FieldNotAllowed, None);
    }
    (Code::InvalidValue, None)
}

/// From "`found`, expected one of `a`, `b`" (or "expected `a` or `b`"), the alternative
/// closest to `found`.
fn suggest_from(rest: &str) -> Option<String> {
    let mut quoted = rest.split('`').skip(1).step_by(2);
    let found = quoted.next()?;
    did_you_mean(found, quoted)
}
