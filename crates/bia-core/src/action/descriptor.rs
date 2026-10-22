//! What an Action is called, takes and returns, as listed in the catalogue.

use schemars::Schema;
use serde::{Deserialize, Serialize, Serializer};

use super::Output;
use crate::dsl::{ActionName, Params};

/// An Action's metadata: the source for validation, `GET /schema` and Dry Run.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ActionDescriptor {
    pub name: ActionName,
    pub description: String,
    pub effect: Effect,
    /// Disabled unless enabled in server config, and only publishable with an `admin` Token
    /// (ADR-0014).
    pub dangerous: bool,
    pub param_schema: Schema,
    pub output_schema: Schema,
    /// Shown to authors, and the source of Mocks in a Dry Run.
    pub examples: Vec<ActionExample>,
}

impl ActionDescriptor {
    pub fn new(name: ActionName, description: impl Into<String>, effect: Effect) -> Self {
        Self {
            name,
            description: description.into(),
            effect,
            dangerous: false,
            param_schema: Schema::default(),
            output_schema: Schema::default(),
            examples: Vec::new(),
        }
    }

    pub fn param_schema(mut self, schema: Schema) -> Self {
        self.param_schema = schema;
        self
    }

    pub fn output_schema(mut self, schema: Schema) -> Self {
        self.output_schema = schema;
        self
    }

    pub fn example(mut self, example: ActionExample) -> Self {
        self.examples.push(example);
        self
    }

    pub fn dangerous(mut self) -> Self {
        self.dangerous = true;
        self
    }

    pub fn category(&self) -> &str {
        self.name.category()
    }
}

impl Serialize for ActionDescriptor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            name: &'a ActionName,
            category: &'a str,
            description: &'a str,
            effect: Effect,
            dangerous: bool,
            param_schema: &'a Schema,
            output_schema: &'a Schema,
            examples: &'a [ActionExample],
        }

        Wire {
            name: &self.name,
            category: self.category(),
            description: &self.description,
            effect: self.effect,
            dangerous: self.dangerous,
            param_schema: &self.param_schema,
            output_schema: &self.output_schema,
            examples: &self.examples,
        }
        .serialize(serializer)
    }
}

/// Whether running an Action can change anything outside the Run. A Dry Run executes `pure`
/// Actions for real and gives `side_effecting` ones a Mock instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Pure,
    SideEffecting,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionExample {
    pub description: String,
    pub params: Params,
    pub output: Output,
}

impl ActionExample {
    pub fn new(description: impl Into<String>, params: Params, output: Output) -> Self {
        Self {
            description: description.into(),
            params,
            output,
        }
    }
}
