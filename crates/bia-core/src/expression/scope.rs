use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use cel::{Context, Value};

use super::convert;

const REDACTED: &str = "[REDACTED]";

/// The immutable values an Expression can read (ADR-0011): `input`,
/// `steps.<id>.output`, `trigger`, `run`, `item` and `index`, `error`, and
/// `secrets.NAME`.
///
/// Only what the caller sets is in scope, except `steps`, which is always
/// there and empty until a Step Output is added. Reading anything else is an
/// [`EvalError`](super::EvalError). Values are converted to CEL once, when
/// they are set, and reused by every evaluation.
#[derive(Clone, Default)]
pub struct Scope {
    variables: BTreeMap<&'static str, Value>,
    steps: BTreeMap<String, Value>,
    secrets: BTreeMap<String, Arc<String>>,
}

impl Scope {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_input(self, input: &serde_json::Value) -> Self {
        self.with_variable("input", input)
    }

    pub fn with_trigger(self, trigger: &serde_json::Value) -> Self {
        self.with_variable("trigger", trigger)
    }

    pub fn with_run(self, run: &serde_json::Value) -> Self {
        self.with_variable("run", run)
    }

    pub fn with_step_output(mut self, id: impl Into<String>, output: &serde_json::Value) -> Self {
        let step = convert::map([("output", convert::from_json(output))]);
        self.steps.insert(id.into(), Value::Map(step));
        self
    }

    pub fn with_item(mut self, item: &serde_json::Value, index: u64) -> Self {
        self.variables.insert("item", convert::from_json(item));
        self.variables
            .insert("index", convert::from_json(&index.into()));
        self
    }

    pub fn with_error(self, error: &serde_json::Value) -> Self {
        self.with_variable("error", error)
    }

    /// The values a `transform.expr` Step hands its Expression, read as
    /// `values`. Only that Action's scope has them.
    pub fn with_values(self, values: &serde_json::Value) -> Self {
        self.with_variable("values", values)
    }

    /// A resolved Secret, read as `secrets.NAME`. Its value is redacted from
    /// every evaluation error and never shown by `Debug`.
    pub fn with_secret(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.secrets.insert(name.into(), Arc::new(value.into()));
        self
    }

    fn with_variable(mut self, name: &'static str, value: &serde_json::Value) -> Self {
        self.variables.insert(name, convert::from_json(value));
        self
    }

    pub(super) fn bind(&self, context: &mut Context<'_, '_>) {
        for (name, value) in &self.variables {
            context.add_variable_from_value(*name, value.clone());
        }
        let steps = convert::map(
            self.steps
                .iter()
                .map(|(id, step)| (id.as_str(), step.clone())),
        );
        context.add_variable_from_value("steps", Value::Map(steps));
        if !self.secrets.is_empty() {
            let secrets = convert::map(
                self.secrets
                    .iter()
                    .map(|(name, value)| (name.as_str(), Value::String(Arc::clone(value)))),
            );
            context.add_variable_from_value("secrets", Value::Map(secrets));
        }
    }

    pub(super) fn redact(&self, text: &str) -> String {
        let mut values: Vec<&str> = self
            .secrets
            .values()
            .map(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .collect();
        // Longest first, so a Secret containing another is redacted whole.
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values
            .into_iter()
            .fold(text.to_owned(), |text, value| text.replace(value, REDACTED))
    }
}

impl fmt::Debug for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scope")
            .field("variables", &self.variables.keys().collect::<Vec<_>>())
            .field("steps", &self.steps.keys().collect::<Vec<_>>())
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}
