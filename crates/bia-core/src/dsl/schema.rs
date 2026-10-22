//! JSON Schema generation for the DSL.

use schemars::generate::SchemaSettings;
use schemars::transform::RecursiveTransform;
use schemars::{JsonSchema, Schema};
use serde_json::{Map, Value};

use super::{Definition, QueueResource};

pub fn definition_schema() -> Value {
    schema_for::<Definition>()
}

pub fn queue_resource_schema() -> Value {
    schema_for::<QueueResource>()
}

fn schema_for<T: JsonSchema>() -> Value {
    SchemaSettings::draft2020_12()
        .with_transform(RecursiveTransform(drop_null))
        .into_generator()
        .into_root_schema_for::<T>()
        .to_value()
}

/// Removes the `null` alternative schemars adds to `Option` fields. The parser treats `null`
/// like an absent field, but the schema only advertises leaving the field out.
fn drop_null(schema: &mut Schema) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    if let Some(Value::Array(types)) = object.get_mut("type") {
        types.retain(|t| t != "null");
        if let [only] = types.as_slice() {
            let only = only.clone();
            object.insert("type".into(), only);
        }
    }
    let is_null = |s: &Value| s.get("type").is_some_and(|t| t == "null");
    let kept = match object.get("anyOf") {
        Some(Value::Array(alternatives)) => match alternatives.as_slice() {
            [Value::Object(kept), null] | [null, Value::Object(kept)] if is_null(null) => {
                kept.clone()
            }
            _ => return,
        },
        _ => return,
    };
    object.remove("anyOf");
    for (key, value) in kept {
        object.entry(key).or_insert(value);
    }
}

/// Builds one variant of a flat object discriminated by `tag`: the `common` properties, the
/// tag, and the variant's own `fields`, with no other properties allowed.
pub(crate) fn tagged_object(
    common: &Schema,
    tag: &str,
    tag_schema: Schema,
    fields: Schema,
) -> Schema {
    let properties_of = |schema: &Schema| match schema.get("properties") {
        Some(Value::Object(properties)) => properties.clone(),
        _ => Map::new(),
    };
    let required_of = |schema: &Schema| match schema.get("required") {
        Some(Value::Array(required)) => required.clone(),
        _ => Vec::new(),
    };

    let mut properties = properties_of(common);
    properties.insert(tag.into(), tag_schema.to_value());
    properties.extend(properties_of(&fields));
    let mut required = required_of(common);
    required.push(tag.into());
    required.extend(required_of(&fields));

    let mut object = Map::new();
    if let Some(description) = fields.get("description") {
        object.insert("description".into(), description.clone());
    }
    object.insert("type".into(), "object".into());
    object.insert("properties".into(), properties.into());
    object.insert("required".into(), required.into());
    object.insert("additionalProperties".into(), false.into());
    Schema::from(object)
}
