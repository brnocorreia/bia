//! The flow Actions that change the Run itself: `flow.sleep`, `flow.sleep_until`, `flow.fail`
//! and `flow.succeed`.
//!
//! They sit in the Action Registry like any Action, so validation, `GET /schema` and Dry Run
//! know their Params, but the interpreter executes them (it sets Run status and schedules
//! Jobs), so [`Flow::execute`] is never called by a Worker. The interpreter reads their
//! Params with [`Control::from_params`].

use chrono::{DateTime, Utc};
use schemars::json_schema;
use serde_json::json;

use super::{
    Action, ActionCall, ActionDescriptor, ActionError, ActionExample, BoxFuture, CancellationToken,
    Effect, Output,
};
use crate::dsl::{ActionName, Duration, Params};

pub const SLEEP: &str = "flow.sleep";
pub const SLEEP_UNTIL: &str = "flow.sleep_until";
pub const FAIL: &str = "flow.fail";
pub const SUCCEED: &str = "flow.succeed";

#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Sleep(Duration),
    SleepUntil(DateTime<Utc>),
    Fail(String),
    Succeed,
}

impl Control {
    /// Reads the resolved Params of the Action called `name`. `None` when `name` is not a flow
    /// Action; an error when the Params are not valid for it.
    pub fn from_params(name: &str, params: &Params) -> Option<Result<Self, ActionError>> {
        let text = |key: &str| match params.get(key) {
            Some(serde_json::Value::String(text)) => Ok(text.as_str()),
            _ => Err(ActionError::fatal(format!("`{key}` must be a string"))),
        };
        let allowed: &[&str] = match name {
            SLEEP => &["duration"],
            SLEEP_UNTIL => &["timestamp"],
            FAIL => &["message"],
            SUCCEED => &[],
            _ => return None,
        };
        if let Some(unknown) = params.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Some(Err(ActionError::fatal(format!(
                "`{name}` takes no `{unknown}` param"
            ))));
        }
        Some(match name {
            SLEEP => text("duration").and_then(|text| {
                text.parse::<Duration>()
                    .map(Self::Sleep)
                    .map_err(|error| ActionError::fatal(format!("`duration` is invalid: {error}")))
            }),
            SLEEP_UNTIL => text("timestamp").and_then(|text| {
                DateTime::parse_from_rfc3339(text)
                    .map(|at| Self::SleepUntil(at.with_timezone(&Utc)))
                    .map_err(|_| {
                        ActionError::fatal("`timestamp` must be an RFC 3339 timestamp".to_owned())
                    })
            }),
            FAIL => text("message").map(|message| Self::Fail(message.to_owned())),
            SUCCEED => Ok(Self::Succeed),
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub struct Flow {
    descriptor: ActionDescriptor,
}

impl Flow {
    pub fn all() -> Vec<Self> {
        let name = |name| ActionName::new(name).expect("a valid Action name");
        let example = |description, params: serde_json::Value, output| {
            let params = match params {
                serde_json::Value::Object(map) => map,
                _ => unreachable!("examples are written as objects"),
            };
            ActionExample::new(description, params, output)
        };
        let descriptors = [
            ActionDescriptor::new(
                name(SLEEP),
                "Pauses the Run for a Duration without holding a Worker. The Run is `sleeping` \
                 meanwhile and continues with the next Step afterwards.",
                Effect::Pure,
            )
            .param_schema(json_schema!({
                "type": "object",
                "title": "flow.sleep params",
                "properties": {
                    "duration": {
                        "type": "string",
                        "pattern": "^[0-9]+(ms|s|m|h|d)$",
                        "description": "How long to sleep: a whole number and a unit, e.g. `30s` or `3d`."
                    }
                },
                "required": ["duration"],
                "additionalProperties": false
            }))
            .output_schema(slept_until())
            .example(example(
                "Wait three days",
                json!({ "duration": "3d" }),
                json!({ "slept_until": "2030-01-04T00:00:00Z" }),
            )),
            ActionDescriptor::new(
                name(SLEEP_UNTIL),
                "Pauses the Run until a moment in time without holding a Worker. A moment that \
                 has already passed does not pause it.",
                Effect::Pure,
            )
            .param_schema(json_schema!({
                "type": "object",
                "title": "flow.sleep_until params",
                "properties": {
                    "timestamp": {
                        "type": "string",
                        "format": "date-time",
                        "description": "When to continue, as an RFC 3339 timestamp; usually a Template such as `{{ input.send_at }}`."
                    }
                },
                "required": ["timestamp"],
                "additionalProperties": false
            }))
            .output_schema(slept_until())
            .example(example(
                "Wait for a date",
                json!({ "timestamp": "2030-01-04T00:00:00Z" }),
                json!({ "slept_until": "2030-01-04T00:00:00Z" }),
            )),
            ActionDescriptor::new(
                name(FAIL),
                "Ends the Run as `failed` with a message, wherever the Step sits.",
                Effect::Pure,
            )
            .param_schema(json_schema!({
                "type": "object",
                "title": "flow.fail params",
                "properties": {
                    "message": { "type": "string", "description": "Why the Run failed." }
                },
                "required": ["message"],
                "additionalProperties": false
            }))
            .output_schema(json_schema!({ "title": "flow.fail output", "type": "null" }))
            .example(example(
                "Reject a case",
                json!({ "message": "Customer is not eligible" }),
                json!(null),
            )),
            ActionDescriptor::new(
                name(SUCCEED),
                "Ends the Run as `succeeded` now, skipping every later Step, even from inside a \
                 Block.",
                Effect::Pure,
            )
            .param_schema(json_schema!({
                "type": "object",
                "title": "flow.succeed params",
                "properties": {},
                "additionalProperties": false
            }))
            .output_schema(json_schema!({ "title": "flow.succeed output", "type": "null" }))
            .example(example("Stop early", json!({}), json!(null))),
        ];
        descriptors
            .into_iter()
            .map(|descriptor| Self { descriptor })
            .collect()
    }
}

fn slept_until() -> schemars::Schema {
    json_schema!({
        "title": "Output",
        "type": "object",
        "properties": {
            "slept_until": { "type": "string", "format": "date-time" }
        },
        "required": ["slept_until"]
    })
}

impl Action for Flow {
    fn descriptor(&self) -> &ActionDescriptor {
        &self.descriptor
    }

    fn execute(
        &self,
        _call: ActionCall,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        let name = self.descriptor.name.clone();
        Box::pin(std::future::ready(Err(ActionError::fatal(format!(
            "`{name}` changes the Run itself, so only the interpreter can execute it"
        )))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(value: serde_json::Value) -> Params {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn params_are_read_into_what_the_run_should_do() {
        let read = |name, value| Control::from_params(name, &params(value));
        assert_eq!(
            read(SLEEP, json!({ "duration": "90m" })),
            Some(Ok(Control::Sleep("90m".parse().unwrap())))
        );
        assert_eq!(
            read(
                SLEEP_UNTIL,
                json!({ "timestamp": "2030-01-01T02:00:00+02:00" })
            ),
            Some(Ok(Control::SleepUntil(
                DateTime::from_timestamp(1_893_456_000, 0).unwrap()
            )))
        );
        assert_eq!(
            read(FAIL, json!({ "message": "no" })),
            Some(Ok(Control::Fail("no".to_owned())))
        );
        assert_eq!(read(SUCCEED, json!({})), Some(Ok(Control::Succeed)));
        assert_eq!(read("http.request", json!({})), None);
    }

    #[test]
    fn bad_params_are_fatal_errors() {
        for (name, value) in [
            (SLEEP, json!({})),
            (SLEEP, json!({ "duration": "1h30m" })),
            (SLEEP, json!({ "duration": 5 })),
            (SLEEP_UNTIL, json!({ "timestamp": "tomorrow" })),
            (FAIL, json!({ "message": "x", "extra": 1 })),
            (SUCCEED, json!({ "message": "x" })),
        ] {
            let result = Control::from_params(name, &params(value.clone()));
            assert!(
                matches!(&result, Some(Err(error)) if !error.is_retryable()),
                "{name} {value}: {result:?}"
            );
        }
    }
}
