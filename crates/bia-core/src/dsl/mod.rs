//! The DSL: typed Definitions and queue resources, their JSON Schema, and JSON/YAML parsing.
//!
//! The target shape is documented in `docs/dsl.md`. Expressions and Templates are kept as
//! opaque strings here; semantic checks (unique ids, scope references, params against the
//! Action) belong to validation.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

mod duration;
mod ids;
mod parse;
mod queue;
mod schema;
mod step;
mod trigger;

pub use duration::{Duration, DurationUnit, InvalidDuration};
pub use ids::{ActionName, InvalidName, QueueName, StepId, TriggerId, WorkflowId};
pub use parse::{ParseError, ParseErrorKind, yaml_to_json};
pub use queue::{QueueKind, QueueResource};
pub use schema::{definition_schema, queue_resource_schema};
pub(crate) use step::BLOCK_ACTIONS;
pub use step::{
    Backoff, Branch, FailureMode, Foreach, Join, LoopUntil, Parallel, Params, RetryPolicy, Step,
    StepKind, Switch, SwitchCase, Try,
};
pub use trigger::{
    CatchUpPolicy, CronTrigger, EventTrigger, Hmac, OverlapPolicy, QueueTrigger, Trigger,
    TriggerKind, WebhookTrigger,
};

/// The document that describes a Version: Triggers, input schema, Steps and error handling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "Definition")]
pub struct Definition {
    pub schema_version: SchemaVersion,
    /// Workflow id, stable across Versions.
    pub id: WorkflowId,
    /// Free text shown in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// What starts a Run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    /// JSON Schema the Trigger payload (`input`) is validated against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Map<String, Value>>,
    /// Per-Workflow limits; global limits apply to anything left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
    /// Steps run in order.
    pub steps: Vec<Step>,
    /// Compensation Steps run when the Run fails.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_error: Vec<Step>,
}

/// Per-Workflow limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Most Runs of this Workflow active at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_concurrent_runs: Option<u32>,
}

/// The DSL version a Definition is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SchemaVersion(u32);

impl SchemaVersion {
    pub const CURRENT: Self = Self(1);

    pub fn new(version: u64) -> Option<Self> {
        (version == u64::from(Self::CURRENT.0)).then_some(Self::CURRENT)
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl Default for SchemaVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let version = u64::deserialize(deserializer)?;
        Self::new(version).ok_or_else(|| {
            serde::de::Error::custom(ParseErrorKind::UnsupportedSchemaVersion {
                found: version.to_string(),
            })
        })
    }
}

impl JsonSchema for SchemaVersion {
    fn schema_name() -> Cow<'static, str> {
        "SchemaVersion".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "The DSL version this Definition is written in.",
            "type": "integer",
            "const": Self::CURRENT.0,
        })
    }
}

/// The source text of a CEL Expression, compiled during validation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Expression(String);

impl Expression {
    pub fn new(source: impl Into<String>) -> Self {
        Self(source.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl JsonSchema for Expression {
    fn schema_name() -> Cow<'static, str> {
        "Expression".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A side-effect-free CEL Expression, written raw (no `{{ }}`).",
            "type": "string",
        })
    }
}

/// Any document bia reads from a file: a Workflow Definition, or a resource told apart by its
/// top-level `kind` (ADR-0025).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Document {
    Workflow(Definition),
    Queue(QueueResource),
}
