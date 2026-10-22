//! Resolving the Templates in a Step's Params at execution time.

use serde_json::Value;

use super::{Limits, Scope, Template};
use crate::dsl::Params;

/// A Step's Params with every Template evaluated, in the two forms the interpreter needs.
#[derive(Clone, PartialEq)]
pub struct ResolvedParams {
    /// What the Action receives. It may hold resolved Secrets, so it is never persisted.
    pub call: Params,
    /// What the Timeline records: `call`, except that every Template reading `secrets` is
    /// kept as written, so no value derived from a Secret is ever stored (ADR-0033).
    pub recorded: Params,
}

// `call` may hold Secrets, so only the recorded form is printed.
impl std::fmt::Debug for ResolvedParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedParams")
            .field("recorded", &self.recorded)
            .finish_non_exhaustive()
    }
}

/// A Template in Params that did not compile or evaluate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Template at '{path}' failed: {message}")]
pub struct ParamsError {
    /// JSON Pointer to the Template, relative to `params`.
    pub path: String,
    /// Why it failed. Never carries a Secret value.
    pub message: String,
}

/// Evaluates every Template in `params` against `scope`. Object keys are never Templates.
pub fn resolve_params(
    params: &Params,
    scope: &Scope,
    limits: &Limits,
) -> Result<ResolvedParams, ParamsError> {
    let mut call = Params::new();
    let mut recorded = Params::new();
    for (key, value) in params {
        let path = format!("/{}", token(key));
        let (for_call, for_record) = resolve(value, scope, limits, &path)?;
        call.insert(key.clone(), for_call);
        recorded.insert(key.clone(), for_record);
    }
    Ok(ResolvedParams { call, recorded })
}

fn resolve(
    value: &Value,
    scope: &Scope,
    limits: &Limits,
    path: &str,
) -> Result<(Value, Value), ParamsError> {
    match value {
        Value::String(source) if source.contains("{{") => {
            let fail = |message: String| ParamsError {
                path: path.to_owned(),
                message,
            };
            let template =
                Template::parse_with(source, limits).map_err(|error| fail(error.message))?;
            let resolved = template
                .evaluate(scope)
                .map_err(|error| fail(error.to_string()))?;
            let references = template.references();
            let reads_secrets = !references.secrets.is_empty() || references.dynamic_secrets;
            let recorded = if reads_secrets {
                value.clone()
            } else {
                resolved.clone()
            };
            Ok((resolved, recorded))
        }
        Value::Array(items) => {
            let mut call = Vec::with_capacity(items.len());
            let mut recorded = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let (for_call, for_record) =
                    resolve(item, scope, limits, &format!("{path}/{index}"))?;
                call.push(for_call);
                recorded.push(for_record);
            }
            Ok((Value::Array(call), Value::Array(recorded)))
        }
        Value::Object(map) => {
            let mut call = serde_json::Map::new();
            let mut recorded = serde_json::Map::new();
            for (key, item) in map {
                let (for_call, for_record) =
                    resolve(item, scope, limits, &format!("{path}/{}", token(key)))?;
                call.insert(key.clone(), for_call);
                recorded.insert(key.clone(), for_record);
            }
            Ok((Value::Object(call), Value::Object(recorded)))
        }
        other => Ok((other.clone(), other.clone())),
    }
}

/// `key` escaped as one JSON Pointer reference token.
fn token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}
