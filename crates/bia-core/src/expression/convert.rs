//! The mapping between JSON values and CEL values (ADR-0026).

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use cel::Value;
use cel::objects::{Key, Map};

/// Converts a JSON value to CEL. A number with no fractional part that fits
/// `int` becomes an `int`, so `steps.a.output.count + 1` works; one that only
/// fits `uint` becomes a `uint`; every other number becomes a `double`.
pub(super) fn from_json(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::UInt(u)
            } else {
                Value::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        serde_json::Value::String(s) => Value::String(Arc::new(s.clone())),
        serde_json::Value::Array(items) => {
            Value::List(Arc::new(items.iter().map(from_json).collect()))
        }
        serde_json::Value::Object(fields) => Value::Map(map(fields
            .iter()
            .map(|(key, value)| (key.as_str(), from_json(value))))),
    }
}

/// Builds a CEL map with string keys.
pub(super) fn map<'k>(entries: impl IntoIterator<Item = (&'k str, Value)>) -> Map {
    let map: HashMap<Key, Value> = entries
        .into_iter()
        .map(|(key, value)| (Key::String(Arc::new(key.to_owned())), value))
        .collect();
    Map { map: Arc::new(map) }
}

/// Converts a CEL value to JSON. `int`, `uint` and finite `double` become
/// numbers, `bytes` a base64 string, `timestamp` an RFC 3339 string,
/// `duration` a seconds string such as `"1.5s"`, and map keys strings.
pub(super) fn to_json(value: &Value) -> Result<serde_json::Value, String> {
    Ok(match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => (*i).into(),
        Value::UInt(u) => (*u).into(),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| format!("the double {f} is not a finite number"))?,
        Value::String(s) => serde_json::Value::String(s.to_string()),
        Value::Bytes(b) => serde_json::Value::String(BASE64.encode(b.as_slice())),
        Value::Timestamp(t) => serde_json::Value::String(t.to_rfc3339()),
        Value::Duration(d) => {
            serde_json::Value::String(duration(d.num_seconds(), d.subsec_nanos()))
        }
        Value::List(items) => {
            serde_json::Value::Array(items.iter().map(to_json).collect::<Result<_, _>>()?)
        }
        Value::Map(map) => {
            let mut object = serde_json::Map::new();
            for (key, value) in map.map.iter() {
                let key = match key {
                    Key::String(s) => s.to_string(),
                    Key::Int(i) => i.to_string(),
                    Key::Uint(u) => u.to_string(),
                    Key::Bool(b) => b.to_string(),
                };
                object.insert(key, to_json(value)?);
            }
            serde_json::Value::Object(object)
        }
        other => return Err(format!("a {} has no JSON form", other.type_of())),
    })
}

/// Formats a duration, given as whole seconds and the nanoseconds left over
/// (both carrying its sign), as CEL's `string(duration)` does: seconds with up
/// to nine fractional digits and an `s` suffix.
fn duration(seconds: i64, nanos: i32) -> String {
    let sign = if seconds < 0 || nanos < 0 { "-" } else { "" };
    let seconds = seconds.unsigned_abs();
    let nanos = nanos.unsigned_abs();
    if nanos == 0 {
        format!("{sign}{seconds}s")
    } else {
        let fraction = format!("{nanos:09}");
        format!("{sign}{seconds}.{}s", fraction.trim_end_matches('0'))
    }
}
