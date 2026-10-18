//! `kind: queue` resource files.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Duration, QueueName};

/// A Queue declared as a resource file (ADR-0015).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "QueueResource")]
pub struct QueueResource {
    pub kind: QueueKind,
    pub name: QueueName,
    /// Deliveries a Message gets before it moves to the Dead-Letter Queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// How long a delivered Message stays hidden from other consumers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_timeout: Option<Duration>,
    /// How many Messages a consumer receives at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub batch_size: Option<u32>,
    /// Queue that receives Messages which exhausted their retries.
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
