//! `transform.expr`: evaluates a CEL Expression over the values it is given.

use bia_core::action::{
    Action, ActionCall, ActionDescriptor, ActionError, ActionExample, BoxFuture, CancellationToken,
    Effect, Output,
};
use bia_core::dsl::ActionName;
use bia_core::expression::{Expression, Scope};
use schemars::{JsonSchema, json_schema};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::params::{ParamReader, object, schema_for};

/// The Params of `transform.expr`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "transform.expr params")]
struct Params {
    /// A CEL Expression. It reads the values as `values`, e.g. `values.items.size()`.
    expression: String,
    /// The values the Expression reads, usually filled from earlier Outputs with Templates.
    #[serde(default)]
    values: Map<String, Value>,
}

/// Evaluates a CEL Expression over the values in its Params. Its Output is the result, of
/// whatever type the Expression produces.
#[derive(Debug)]
pub struct Expr {
    descriptor: ActionDescriptor,
    params: ParamReader,
}

impl Expr {
    pub fn new() -> Self {
        let descriptor = ActionDescriptor::new(
            ActionName::new("transform.expr").expect("a valid Action name"),
            "Evaluates a CEL Expression over `values` and returns its result: a number, string, \
             boolean, list, object or null. Use it to compute or reshape data between Steps.",
            Effect::Pure,
        )
        .param_schema(schema_for::<Params>())
        .output_schema(json_schema!({
            "title": "transform.expr output",
            "description": "The value the Expression evaluates to."
        }))
        .example(ActionExample::new(
            "Compute an order total",
            object(json!({
                "expression": "values.price * double(values.quantity)",
                "values": { "price": 12.5, "quantity": 4 }
            })),
            json!(50.0),
        ))
        .example(ActionExample::new(
            "Keep the names of active users",
            object(json!({
                "expression": "values.users.filter(u, u.active).map(u, u.name)",
                "values": {
                    "users": [
                        { "name": "ada", "active": true },
                        { "name": "bob", "active": false }
                    ]
                }
            })),
            json!(["ada"]),
        ))
        .example(ActionExample::new(
            "Build an object",
            object(json!({
                "expression": "{'id': values.id, 'tags': values.tags.size()}",
                "values": { "id": "ord_1", "tags": ["new", "vip"] }
            })),
            json!({ "id": "ord_1", "tags": 2 }),
        ));
        let params = ParamReader::new(&descriptor);
        Self { descriptor, params }
    }
}

impl Default for Expr {
    fn default() -> Self {
        Self::new()
    }
}

impl Action for Expr {
    fn descriptor(&self) -> &ActionDescriptor {
        &self.descriptor
    }

    fn execute(
        &self,
        call: ActionCall,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        // Evaluation is bounded by the Expression cost limit, so it finishes quickly and does
        // not need to watch for cancellation.
        let result = self.params.read::<Params>(&call.params).and_then(|params| {
            let expression = Expression::compile(&params.expression).map_err(|error| {
                ActionError::fatal(format!("`expression` does not compile: {error}"))
            })?;
            let scope = Scope::new().with_values(&Value::Object(params.values));
            expression.evaluate(&scope).map_err(|error| {
                ActionError::fatal(format!("`expression` failed to evaluate: {error}"))
            })
        });
        Box::pin(std::future::ready(result))
    }
}
