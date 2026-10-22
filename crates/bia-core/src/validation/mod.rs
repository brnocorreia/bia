//! Layered validation of Definitions into Diagnostics (ADR-0001).
//!
//! Validation runs in layers: the schema layer turns a JSON value into a typed [`Definition`],
//! and the semantic layer then checks it as a whole (unique ids, scope references, Expressions,
//! Params against their Action, referenced Queues and Workflows, disabled Actions, limits).
//! The semantic layer reports every problem it finds in one pass. A document that fails the
//! schema layer has no typed form to check further, so it yields that layer's Diagnostic alone.
//!
//! Checks that depend on the world go through the injected [`ActionRegistry`] and
//! [`ResourceLookup`], so validation itself stays pure.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::action::ActionRegistry;
use crate::dsl::{Definition, QueueName, WorkflowId};

mod code;
mod instance;
mod params;
mod semantic;
mod shape;
mod suggest;

pub use code::Code;
pub use instance::{InvalidSchema, SchemaViolation, schema_violations};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub path: String,
    pub code: Code,
    pub severity: Severity,
    /// Human-readable explanation. Its wording may change.
    pub message: String,
    #[serde(default)]
    pub suggestion: Option<String>,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// How serious a Diagnostic is. Only errors stop a Version from being published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// What the Definition is being validated for. Some problems that a Draft may still have are
/// errors at publish time (ADR-0014, ADR-0015).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Draft,
    Publish,
}

impl Target {
    /// The severity of a problem that is tolerated on a Draft but blocks publishing.
    fn publish_blocker(self) -> Severity {
        match self {
            Self::Draft => Severity::Warning,
            Self::Publish => Severity::Error,
        }
    }
}

/// Resources a Definition can reference by name, as known to the caller.
///
/// `bia-core` does no IO, so callers load what they need beforehand and answer from memory.
pub trait ResourceLookup {
    fn queue_exists(&self, name: &str) -> bool;

    fn queue_names(&self) -> Vec<String>;

    fn workflow_exists(&self, id: &str) -> bool;

    fn workflow_ids(&self) -> Vec<String>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownResources {
    pub queues: BTreeSet<String>,
    pub workflows: BTreeSet<String>,
}

impl KnownResources {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn queue(mut self, name: QueueName) -> Self {
        self.queues.insert(name.as_str().to_owned());
        self
    }

    pub fn workflow(mut self, id: WorkflowId) -> Self {
        self.workflows.insert(id.as_str().to_owned());
        self
    }
}

impl ResourceLookup for KnownResources {
    fn queue_exists(&self, name: &str) -> bool {
        self.queues.contains(name)
    }

    fn queue_names(&self) -> Vec<String> {
        self.queues.iter().cloned().collect()
    }

    fn workflow_exists(&self, id: &str) -> bool {
        self.workflows.contains(id)
    }

    fn workflow_ids(&self) -> Vec<String> {
        self.workflows.iter().cloned().collect()
    }
}

/// Configured bounds a Definition must stay within, so a Workflow can't run away (ADR-0012).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_steps: usize,
    pub max_nesting_depth: usize,
    pub max_iterations: u32,
    pub expression: crate::expression::Limits,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_steps: 500,
            max_nesting_depth: 16,
            max_iterations: 10_000,
            expression: crate::expression::Limits::default(),
        }
    }
}

pub struct Validator<'a> {
    target: Target,
    actions: &'a dyn ActionRegistry,
    resources: &'a dyn ResourceLookup,
    limits: Limits,
}

impl<'a> Validator<'a> {
    pub fn new(
        target: Target,
        actions: &'a dyn ActionRegistry,
        resources: &'a dyn ResourceLookup,
    ) -> Self {
        Self {
            target,
            actions,
            resources,
            limits: Limits::default(),
        }
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub fn validate_value(&self, value: Value) -> Report {
        match Definition::from_value(value) {
            Ok(definition) => {
                let diagnostics = self.validate(&definition);
                Report {
                    definition: Some(definition),
                    diagnostics,
                }
            }
            Err(error) => Report {
                definition: None,
                diagnostics: vec![error.into()],
            },
        }
    }

    /// Runs the semantic layer on an already parsed Definition and returns every Diagnostic,
    /// in document order.
    pub fn validate(&self, definition: &Definition) -> Vec<Diagnostic> {
        semantic::check(self, definition)
    }
}

impl std::fmt::Debug for Validator<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Validator")
            .field("target", &self.target)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub definition: Option<Definition>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    /// Whether any Diagnostic is an error. Warnings alone don't block publishing.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }
}

fn token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}
