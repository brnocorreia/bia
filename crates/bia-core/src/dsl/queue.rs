//! `kind: queue` resource files.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Duration, DurationUnit, QueueName};

pub const DEFAULT_MAX_RETRIES: u32 = 3;
pub const DEFAULT_BATCH_SIZE: u32 = 10;
/// The most Messages one delivery or one `queue.publish_batch` handles.
pub const MAX_BATCH_SIZE: u32 = 100;

pub fn default_visibility_timeout() -> Duration {
    Duration::new(30, DurationUnit::Seconds).expect("30s fits")
}

/// A Queue declared as a resource file (ADR-0015). Every setting is optional; applying the
/// file again replaces all of them, so a setting left out goes back to its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "QueueResource")]
pub struct QueueResource {
    pub kind: QueueKind,
    pub name: QueueName,
    /// How many more times a Message is delivered after its first delivery fails or is not
    /// acked within the visibility timeout, before it moves to the Dead-Letter Queue. Default
    /// 3; `0` dead-letters a Message after one failed delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// How long a delivered Message stays hidden from other consumers before it is delivered
    /// again, unless it is acked first. Default `30s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_timeout: Option<Duration>,
    /// How many Messages a consumer receives at once, at most 100. Default 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 100))]
    pub batch_size: Option<u32>,
    /// The Queue that receives Messages which exhausted their retries. It must exist before
    /// this Queue names it. Without one, exhausted Messages stay in this Queue as dead
    /// letters and are not delivered again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dead_letter: Option<QueueName>,
}

/// The `kind` of a queue resource file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueueKind {
    #[default]
    Queue,
}
