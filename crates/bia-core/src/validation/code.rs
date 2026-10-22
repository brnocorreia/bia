//! The catalogue of Diagnostic codes, a public contract for agents (ADR-0028).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! codes {
    ($( $(#[doc = $doc:literal])+ $variant:ident => $code:literal, )+) => {
        /// A stable identifier for a kind of Diagnostic. Codes may be added; renaming or
        /// removing one needs an ADR.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[non_exhaustive]
        pub enum Code {
            $( $(#[doc = $doc])+ $variant, )+
        }

        impl Code {
            pub const ALL: &[Self] = &[$( Self::$variant, )+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $code, )+
                }
            }
        }
    };
}

codes! {
    /// The text is not well-formed JSON or YAML.
    InvalidSyntax => "invalid_syntax",
    /// The Definition has no `schema_version`.
    MissingSchemaVersion => "missing_schema_version",
    /// The Definition's `schema_version` is not supported by this server.
    UnsupportedSchemaVersion => "unsupported_schema_version",
    /// A document's `kind` names no known resource.
    UnknownKind => "unknown_kind",
    /// An object has a field the DSL does not define.
    UnknownField => "unknown_field",
    /// A required field is absent.
    MissingField => "missing_field",
    /// A field exists in the DSL but not on this Action or Trigger type.
    FieldNotAllowed => "field_not_allowed",
    /// A value has the wrong type or format, such as a malformed id or Duration.
    InvalidValue => "invalid_value",

    /// Two Steps share an id.
    DuplicateStepId => "duplicate_step_id",
    /// Two Triggers share an id.
    DuplicateTriggerId => "duplicate_trigger_id",
    /// An Expression or Template does not compile.
    InvalidExpression => "invalid_expression",
    /// An Expression is longer or nests deeper than the configured limit.
    ExpressionLimitExceeded => "expression_limit_exceeded",
    /// `secrets.*` is read outside `params`.
    SecretOutsideParams => "secret_outside_params",
    /// `item`, `index` or `error` is read where it is not in scope.
    VariableOutOfScope => "variable_out_of_scope",
    /// `steps.<id>` names no Step of the Definition.
    UnknownStepReference => "unknown_step_reference",
    /// `steps.<id>` names a Step that has not finished at this point.
    ForwardStepReference => "forward_step_reference",
    /// `steps.<id>` names an earlier Step that does not run on every path to this point.
    StepMayNotHaveRun => "step_may_not_have_run",
    /// The Action is not in the Action Registry.
    UnknownAction => "unknown_action",
    /// The Action is known but disabled on this server.
    DisabledAction => "disabled_action",
    /// Params do not match the Action's param schema.
    InvalidParams => "invalid_params",
    /// A referenced Queue does not exist.
    UnknownQueue => "unknown_queue",
    /// A referenced Workflow does not exist.
    UnknownWorkflow => "unknown_workflow",
    /// The Definition has more Steps than the configured limit.
    StepLimitExceeded => "step_limit_exceeded",
    /// A Step sits inside more Blocks than the configured limit.
    NestingLimitExceeded => "nesting_limit_exceeded",
    /// A loop declares more iterations than the configured limit.
    IterationLimitExceeded => "iteration_limit_exceeded",
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Code {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Code {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Self::ALL
            .iter()
            .copied()
            .find(|known| known.as_str() == code)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown Diagnostic code `{code}`")))
    }
}
