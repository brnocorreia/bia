//! Checking a value against a JSON Schema at execution time: a Run's input against the
//! Definition's `input_schema`, and resolved Params against their Action's param schema.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaViolation {
    pub path: String,
    /// What is wrong. It never echoes the value, which may be sensitive.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the JSON Schema is invalid: {0}")]
pub struct InvalidSchema(pub String);

pub fn schema_violations(
    schema: &Value,
    value: &Value,
) -> Result<Vec<SchemaViolation>, InvalidSchema> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| InvalidSchema(error.masked().to_string()))?;
    Ok(validator
        .iter_errors(value)
        .map(|error| SchemaViolation {
            path: error.instance_path().to_string(),
            message: error.masked().to_string(),
        })
        .collect())
}
