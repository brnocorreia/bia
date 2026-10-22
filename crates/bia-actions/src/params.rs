//! Turning resolved Params into an Action's typed Params.

use bia_core::action::{ActionDescriptor, ActionError};
use bia_core::dsl::Params;
use jsonschema::Validator;
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Debug)]
pub(crate) struct ParamReader {
    validator: Validator,
}

impl ParamReader {
    /// # Panics
    ///
    /// When the descriptor's param schema does not compile, which is a bug in the Action.
    pub(crate) fn new(descriptor: &ActionDescriptor) -> Self {
        let validator = jsonschema::validator_for(descriptor.param_schema.as_value())
            .unwrap_or_else(|e| panic!("`{}` has an invalid param schema: {e}", descriptor.name));
        Self { validator }
    }

    /// Invalid Params are `fatal`: every Attempt would get the same ones. Messages never echo
    /// values, which may be resolved Secrets.
    pub(crate) fn read<T: DeserializeOwned>(&self, params: &Params) -> Result<T, ActionError> {
        let instance = Value::Object(params.clone());
        let problems: Vec<String> = self
            .validator
            .iter_errors(&instance)
            .map(|error| {
                let location = error.instance_path().to_string();
                let location = if location.is_empty() { "/" } else { &location };
                format!("{location}: {}", error.masked())
            })
            .collect();
        if !problems.is_empty() {
            return Err(
                ActionError::fatal(format!("invalid Params: {}", problems.join("; ")))
                    .with_details(serde_json::json!({ "problems": problems })),
            );
        }
        serde_json::from_value(instance)
            .map_err(|_| ActionError::fatal("invalid Params: they do not match the param schema"))
    }
}

pub(crate) fn schema_for<T: schemars::JsonSchema>() -> schemars::Schema {
    schemars::SchemaGenerator::new(schemars::generate::SchemaSettings::draft2020_12())
        .into_root_schema_for::<T>()
}

pub(crate) fn object(value: Value) -> bia_core::dsl::Params {
    match value {
        Value::Object(map) => map,
        _ => unreachable!("examples are written as objects"),
    }
}
