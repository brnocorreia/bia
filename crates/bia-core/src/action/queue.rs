//! `queue.publish` and `queue.publish_batch`: the Actions that add Messages to a Queue.
//!
//! They sit in the Action Registry like any Action, so validation, `GET /v1/schema` and Dry Run
//! know their Params, but the interpreter publishes their Messages in the transaction that
//! records the Step's Attempt, so a Message exists exactly when the Step succeeded and a
//! retried Attempt cannot publish it twice (ADR-0059). [`QueueAction::execute`] is never called
//! by a Worker. The interpreter reads their Params with [`Publish::from_params`].

use schemars::json_schema;
use serde_json::{Value, json};

use super::{
    Action, ActionCall, ActionDescriptor, ActionError, ActionExample, BoxFuture, CancellationToken,
    Effect, Output,
};
use crate::dsl::{ActionName, Duration, MAX_BATCH_SIZE, Params, QueueName};

pub const PUBLISH: &str = "queue.publish";
pub const PUBLISH_BATCH: &str = "queue.publish_batch";

/// The largest Message body, in bytes of compact JSON.
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024;

/// Messages to add to one Queue, as a Step's resolved Params ask.
#[derive(Debug, Clone, PartialEq)]
pub struct Publish {
    pub queue: QueueName,
    pub bodies: Vec<Value>,
    /// How long the Messages stay unclaimable after the Step succeeds.
    pub delay: Option<Duration>,
}

impl Publish {
    /// Reads the resolved Params of the Action called `name`. `None` when `name` is not a
    /// queue Action; a `fatal` error when the Params are not valid for it, including a body
    /// over [`MAX_MESSAGE_BYTES`].
    pub fn from_params(name: &str, params: &Params) -> Option<Result<Self, ActionError>> {
        let (list, allowed): (&str, &[&str]) = match name {
            PUBLISH => ("body", &["queue", "body", "delay"]),
            PUBLISH_BATCH => ("bodies", &["queue", "bodies", "delay"]),
            _ => return None,
        };
        Some(Self::read(name, params, list, allowed))
    }

    fn read(
        name: &str,
        params: &Params,
        list: &str,
        allowed: &[&str],
    ) -> Result<Self, ActionError> {
        if let Some(unknown) = params.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(ActionError::fatal(format!(
                "`{name}` takes no `{unknown}` param"
            )));
        }
        let queue = match params.get("queue") {
            Some(Value::String(queue)) => QueueName::new(queue.as_str())
                .map_err(|error| ActionError::fatal(format!("`queue` is invalid: {error}")))?,
            _ => return Err(ActionError::fatal("`queue` must be a string")),
        };
        let bodies = match (list, params.get(list)) {
            ("body", Some(body)) => vec![body.clone()],
            ("bodies", Some(Value::Array(bodies))) => bodies.clone(),
            _ if list == "body" => return Err(ActionError::fatal("`body` is required")),
            _ => return Err(ActionError::fatal("`bodies` must be an array")),
        };
        let max = usize::try_from(MAX_BATCH_SIZE).unwrap_or(usize::MAX);
        if bodies.is_empty() || bodies.len() > max {
            return Err(ActionError::fatal(format!(
                "`bodies` holds {} Messages; it takes 1 to {max}",
                bodies.len()
            )));
        }
        for (index, body) in bodies.iter().enumerate() {
            let size = serde_json::to_vec(body).map_or(usize::MAX, |bytes| bytes.len());
            if size > MAX_MESSAGE_BYTES {
                let at = if list == "body" {
                    "`body`".to_owned()
                } else {
                    format!("`bodies[{index}]`")
                };
                return Err(ActionError::fatal(format!(
                    "{at} is {size} bytes of JSON; a Message body is at most {MAX_MESSAGE_BYTES}"
                ))
                .with_details(json!({ "max": MAX_MESSAGE_BYTES, "actual": size })));
            }
        }
        let delay = match params.get("delay") {
            None | Some(Value::Null) => None,
            Some(Value::String(delay)) => Some(
                delay
                    .parse::<Duration>()
                    .map_err(|error| ActionError::fatal(format!("`delay` is invalid: {error}")))?,
            ),
            Some(_) => return Err(ActionError::fatal("`delay` must be a Duration string")),
        };
        Ok(Self {
            queue,
            bodies,
            delay,
        })
    }
}

#[derive(Debug)]
pub struct QueueAction {
    descriptor: ActionDescriptor,
}

impl QueueAction {
    pub fn all() -> Vec<Self> {
        let name = |name| ActionName::new(name).expect("a valid Action name");
        let example = |description, params: Value, output| {
            let Value::Object(params) = params else {
                unreachable!("examples are written as objects")
            };
            ActionExample::new(description, params, output)
        };
        let queue = json!({
            "type": "string",
            "pattern": "^[a-z][a-z0-9_-]*$",
            "maxLength": 64,
            "x-bia-resource": "queue",
            "description": "The Queue to publish to. It must exist: Queues are created explicitly, never on first publish."
        });
        let delay = json!({
            "type": "string",
            "pattern": "^(0|[1-9][0-9]*)(ms|s|m|h|d)$",
            "description": "How long the Messages stay unclaimable after the Step succeeds, e.g. `10m`. Default: claimable at once."
        });
        let descriptors = [
            ActionDescriptor::new(
                name(PUBLISH),
                "Adds one Message to a Queue, optionally delayed. The Message is published when \
                 the Step succeeds, together with its Output, so retries never publish it twice.",
                Effect::SideEffecting,
            )
            .param_schema(json_schema!({
                "type": "object",
                "title": "queue.publish params",
                "properties": {
                    "queue": queue,
                    "body": { "description": "The Message: any JSON value, at most 256 KiB as compact JSON." },
                    "delay": delay,
                },
                "required": ["queue", "body"],
                "additionalProperties": false
            }))
            .output_schema(published())
            .example(example(
                "Hand a signup to the email sender",
                json!({ "queue": "emails", "body": { "email": "ada@example.com", "template": "welcome" } }),
                json!({ "queue": "emails", "published": 1, "visible_at": "2030-01-01T00:00:00Z" }),
            )),
            ActionDescriptor::new(
                name(PUBLISH_BATCH),
                "Adds up to 100 Messages to a Queue in order, optionally delayed. They are \
                 published when the Step succeeds, all together, so retries never publish them \
                 twice.",
                Effect::SideEffecting,
            )
            .param_schema(json_schema!({
                "type": "object",
                "title": "queue.publish_batch params",
                "properties": {
                    "queue": queue,
                    "bodies": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_BATCH_SIZE,
                        "description": "One Message per item, each any JSON value of at most 256 KiB as compact JSON; usually a Template such as `{{ steps.fetch.output.body.users }}`."
                    },
                    "delay": delay,
                },
                "required": ["queue", "bodies"],
                "additionalProperties": false
            }))
            .output_schema(published())
            .example(example(
                "Fan reminders out, an hour from now",
                json!({ "queue": "reminders", "bodies": [{ "user": 1 }, { "user": 2 }], "delay": "1h" }),
                json!({ "queue": "reminders", "published": 2, "visible_at": "2030-01-01T01:00:00Z" }),
            )),
        ];
        descriptors
            .into_iter()
            .map(|descriptor| Self { descriptor })
            .collect()
    }
}

fn published() -> schemars::Schema {
    json_schema!({
        "title": "Output",
        "type": "object",
        "properties": {
            "queue": { "type": "string" },
            "published": { "type": "integer", "description": "How many Messages were published." },
            "visible_at": { "type": "string", "format": "date-time", "description": "When consumers can first receive them." }
        },
        "required": ["queue", "published", "visible_at"]
    })
}

impl Action for QueueAction {
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
            "`{name}` publishes with the Step's own transaction, so only the interpreter can \
             execute it"
        )))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(name: &str, value: Value) -> Option<Result<Publish, ActionError>> {
        Publish::from_params(name, value.as_object().unwrap_or(&Params::new()))
    }

    #[test]
    fn params_are_read_into_the_messages_to_publish() {
        let one = read(PUBLISH, json!({ "queue": "emails", "body": { "a": 1 } }));
        assert_eq!(
            one,
            Some(Ok(Publish {
                queue: QueueName::new("emails").unwrap(),
                bodies: vec![json!({ "a": 1 })],
                delay: None,
            }))
        );
        let batch = read(
            PUBLISH_BATCH,
            json!({ "queue": "emails", "bodies": [1, null], "delay": "5m" }),
        );
        assert_eq!(
            batch,
            Some(Ok(Publish {
                queue: QueueName::new("emails").unwrap(),
                bodies: vec![json!(1), Value::Null],
                delay: Some("5m".parse().unwrap()),
            }))
        );
        assert_eq!(read("http.request", json!({})), None);
    }

    #[test]
    fn bad_params_and_oversized_bodies_are_fatal_errors() {
        let huge = "x".repeat(MAX_MESSAGE_BYTES);
        for (name, value) in [
            (PUBLISH, json!({ "queue": "emails" })),
            (PUBLISH, json!({ "queue": "Emails", "body": 1 })),
            (
                PUBLISH,
                json!({ "queue": "emails", "body": 1, "delay": "soon" }),
            ),
            (PUBLISH, json!({ "queue": "emails", "body": huge })),
            (
                PUBLISH,
                json!({ "queue": "emails", "body": 1, "bodies": [] }),
            ),
            (PUBLISH_BATCH, json!({ "queue": "emails", "bodies": [] })),
            (
                PUBLISH_BATCH,
                json!({ "queue": "emails", "bodies": vec![1; 101] }),
            ),
            (PUBLISH_BATCH, json!({ "queue": "emails", "bodies": "x" })),
        ] {
            let result = read(name, value.clone());
            assert!(
                matches!(&result, Some(Err(error)) if !error.is_retryable()),
                "{name} {value}: {result:?}"
            );
        }
    }
}
