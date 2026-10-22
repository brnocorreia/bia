//! Params against an Action's descriptor: its param schema, and the Queues and Workflows its
//! Params name (ADR-0029).

use jsonschema::error::ValidationErrorKind;
use serde_json::Value;

use super::token;
use crate::action::ActionDescriptor;
use crate::dsl::Params;

/// The schema keyword that marks a param as naming a resource (ADR-0029).
pub(super) const RESOURCE_KEYWORD: &str = "x-bia-resource";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Resource {
    Queue,
    Workflow,
}

#[derive(Debug)]
pub(super) struct Mismatch {
    pub(super) location: String,
    pub(super) message: String,
}

#[derive(Debug)]
pub(super) struct ResourceRef {
    pub(super) resource: Resource,
    pub(super) name: String,
    pub(super) location: String,
}

/// Where Params break the param schema.
///
/// Templates resolve only at execution time, so a value written as a Template, and anything an
/// Expression could still change, is not held against the schema. Messages never echo values,
/// which may be sensitive.
pub(super) fn mismatches(descriptor: &ActionDescriptor, params: &Params) -> Vec<Mismatch> {
    // A schema that does not compile is the Action's fault, not the author's, and is caught
    // when the Action is registered.
    let Ok(validator) = jsonschema::validator_for(descriptor.param_schema.as_value()) else {
        return Vec::new();
    };
    let instance = Value::Object(params.clone());
    validator
        .iter_errors(&instance)
        .filter_map(|error| {
            let location = error.instance_path().to_string();
            let target = instance.pointer(&location)?;
            let combinator = matches!(
                error.kind(),
                ValidationErrorKind::AnyOf { .. }
                    | ValidationErrorKind::OneOfNotValid { .. }
                    | ValidationErrorKind::OneOfMultipleValid { .. }
                    | ValidationErrorKind::Not { .. }
            );
            let unresolved =
                templated_along(&instance, &location) || (combinator && contains_template(target));
            (!unresolved).then(|| Mismatch {
                location,
                message: error.masked().to_string(),
            })
        })
        .collect()
}

/// Every literal resource name in `params`, with its pointer relative to `params`.
///
/// A param names a resource when its schema, reached through `properties` from the root of
/// the param schema, carries `x-bia-resource: queue` or `x-bia-resource: workflow`. Names
/// written as Templates are only known at execution time.
pub(super) fn resources(descriptor: &ActionDescriptor, params: &Params) -> Vec<ResourceRef> {
    let mut found = Vec::new();
    collect(
        descriptor.param_schema.as_value(),
        &Value::Object(params.clone()),
        String::new(),
        &mut found,
    );
    found
}

fn collect(schema: &Value, value: &Value, location: String, found: &mut Vec<ResourceRef>) {
    let resource = match schema.get(RESOURCE_KEYWORD).and_then(Value::as_str) {
        Some("queue") => Some(Resource::Queue),
        Some("workflow") => Some(Resource::Workflow),
        _ => None,
    };
    if let (Some(resource), Value::String(name)) = (resource, value)
        && !name.contains("{{")
    {
        found.push(ResourceRef {
            resource,
            name: name.clone(),
            location: location.clone(),
        });
    }
    if let (Some(Value::Object(properties)), Value::Object(object)) =
        (schema.get("properties"), value)
    {
        for (key, property) in properties {
            if let Some(child) = object.get(key) {
                collect(property, child, format!("{location}/{}", token(key)), found);
            }
        }
    }
}

fn templated_along(instance: &Value, location: &str) -> bool {
    let mut current = instance;
    let tokens = location.split('/').skip(1);
    for token in tokens {
        if is_template(current) {
            return true;
        }
        let token = token.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::Object(map) => match map.get(&token) {
                Some(next) => next,
                None => return false,
            },
            Value::Array(items) => match token.parse::<usize>().ok().and_then(|i| items.get(i)) {
                Some(next) => next,
                None => return false,
            },
            _ => return false,
        };
    }
    is_template(current)
}

fn contains_template(value: &Value) -> bool {
    match value {
        Value::String(_) => is_template(value),
        Value::Array(items) => items.iter().any(contains_template),
        Value::Object(map) => map.values().any(contains_template),
        _ => false,
    }
}

fn is_template(value: &Value) -> bool {
    matches!(value, Value::String(s) if s.contains("{{"))
}
