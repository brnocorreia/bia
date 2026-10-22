use cel::{ExecutionError, ParseErrors};

use super::scope::Scope;

/// What kind of problem stopped an Expression or Template from compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompileErrorKind {
    /// The source is not valid CEL.
    Syntax,
    /// The Expression, or a `{{ }}` segment, has nothing in it.
    Empty,
    /// The Expression is longer than [`Limits::max_source_len`](super::Limits).
    TooLong,
    /// The Expression nests deeper than
    /// [`Limits::max_nesting_depth`](super::Limits).
    TooDeep,
    /// A Template opens `{{` and never closes it.
    UnclosedTemplate,
}

/// An Expression or Template that does not compile, with the position of the
/// problem in its source.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} (line {line}, column {column})")]
pub struct CompileError {
    pub kind: CompileErrorKind,
    pub message: String,
    /// Byte offset into the source.
    pub offset: usize,
    pub line: usize,
    /// 1-based column, counted in characters.
    pub column: usize,
}

impl CompileError {
    pub(super) fn new(
        kind: CompileErrorKind,
        message: impl Into<String>,
        source: &str,
        offset: usize,
    ) -> Self {
        let (line, column) = line_column(source, offset);
        Self {
            kind,
            message: message.into(),
            offset,
            line,
            column,
        }
    }

    /// The first of cel's parse errors, which carry a 1-based line and a
    /// 1-based column in bytes. Errors about the parse as a whole have no
    /// position (line 0) and sort first, so a positioned one is preferred.
    pub(super) fn from_parse(source: &str, errors: &ParseErrors) -> Self {
        if errors
            .errors
            .iter()
            .any(|error| error.msg.starts_with("expression recursion limit exceeded"))
        {
            return Self::new(
                CompileErrorKind::TooDeep,
                "Expression nests deeper than the parser accepts",
                source,
                0,
            );
        }
        let Some(error) = errors
            .errors
            .iter()
            .find(|error| error.pos.0 > 0)
            .or(errors.errors.first())
        else {
            return Self::new(CompileErrorKind::Syntax, "invalid Expression", source, 0);
        };
        let line = usize::try_from(error.pos.0).unwrap_or(1).max(1);
        let column = usize::try_from(error.pos.1).unwrap_or(1);
        let offset = offset_of(source, line, column);
        Self::new(CompileErrorKind::Syntax, error.msg.clone(), source, offset)
    }

    /// Moves this error, found in a segment starting at byte `base` of
    /// `source`, to its position in `source`.
    pub(super) fn shifted(self, source: &str, base: usize) -> Self {
        Self::new(self.kind, self.message, source, base + self.offset)
    }
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let before = source.get(..offset).unwrap_or(source);
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = before[line_start..].chars().count() + 1;
    (line, column)
}

/// The byte offset of a 1-based line and 1-based byte column.
fn offset_of(source: &str, line: usize, column: usize) -> usize {
    let line_start: usize = source
        .split_inclusive('\n')
        .take(line - 1)
        .map(str::len)
        .sum();
    let mut offset = (line_start + column.saturating_sub(1)).min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// An Expression that compiled but failed to evaluate.
///
/// Messages never carry Secret values: anything that could echo one is
/// redacted against the [`Scope`] the Expression ran in.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EvalError {
    /// The Expression names a variable the scope does not have, such as
    /// `item` outside `flow.foreach`.
    #[error("unknown variable '{name}'")]
    UnknownVariable { name: String },
    /// The Expression reads a key or field that is not there, such as
    /// `steps.<id>` for a Step that has not run.
    #[error("no such key '{key}'")]
    NoSuchKey { key: String },
    /// A condition evaluated to something other than a boolean.
    #[error("expected a bool, got {got}")]
    NotABoolean { got: String },
    /// The evaluation spent more than [`Limits::max_cost`](super::Limits).
    #[error("evaluation exceeded the cost limit of {limit}")]
    CostLimitExceeded { limit: u64 },
    /// The result has no JSON form, such as a non-finite double.
    #[error("result is not representable as JSON: {reason}")]
    NotJson { reason: String },
    /// Any other runtime failure: a type mismatch, a division by zero, a
    /// function error.
    #[error("{message}")]
    Failed { message: String },
}

impl EvalError {
    pub(super) fn from_execution(error: ExecutionError, scope: &Scope) -> Self {
        // Messages are built from types rather than values where cel's own
        // would print a value, which could be a Secret.
        let message = match error {
            ExecutionError::UndeclaredReference(name) => {
                return Self::UnknownVariable {
                    name: scope.redact(&name),
                };
            }
            ExecutionError::NoSuchKey(key) => {
                return Self::NoSuchKey {
                    key: scope.redact(&key),
                };
            }
            ExecutionError::UnexpectedType { got, want } => format!("expected {want}, got {got}"),
            ExecutionError::NoSuchOverload(overload) => overload.to_string(),
            ExecutionError::ValuesNotComparable(a, b) => {
                format!("a {} cannot be compared to a {}", a.type_of(), b.type_of())
            }
            ExecutionError::UnsupportedBinaryOperator(op, a, b) => format!(
                "operator '{op}' does not apply to {} and {}",
                a.type_of(),
                b.type_of()
            ),
            ExecutionError::UnsupportedIndex(index, target) => format!(
                "a {} cannot be indexed by a {}",
                target.type_of(),
                index.type_of()
            ),
            ExecutionError::UnsupportedKeyType(key) => {
                format!("a {} cannot be a map key", key.type_of())
            }
            ExecutionError::UnsupportedTargetType { target } => {
                format!("unsupported argument of type {}", target.type_of())
            }
            ExecutionError::NotSupportedAsMethod { method, target } => {
                format!("'{method}' is not a method of {}", target.type_of())
            }
            ExecutionError::DuplicateKey(_) => "map literal repeats a key".to_owned(),
            ExecutionError::DivisionByZero(_) => "division by zero".to_owned(),
            ExecutionError::RemainderByZero(_) => "modulo by zero".to_owned(),
            ExecutionError::Overflow(op, _, _) => format!("overflow in operator '{op}'"),
            ExecutionError::IndexOutOfBounds(_) => "index out of bounds".to_owned(),
            other => other.to_string(),
        };
        Self::Failed {
            message: scope.redact(&message),
        }
    }
}
