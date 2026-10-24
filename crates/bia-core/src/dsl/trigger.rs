//! Triggers: what starts a Run.
//!
//! Like Steps, a Trigger is one flat object discriminated by `type`, read in one pass so
//! errors keep their JSON path.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize, Serializer};

use super::schema::tagged_object;
use super::{Duration, QueueName, TriggerId};
use crate::cron::{Cron, CronError};

/// A declaration of what starts a Run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawTrigger")]
pub struct Trigger {
    pub id: TriggerId,
    pub kind: TriggerKind,
}

/// The `type` of a Trigger and its fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerKind {
    Cron(CronTrigger),
    Queue(QueueTrigger),
    Webhook(WebhookTrigger),
    Manual,
    Event(EventTrigger),
}

impl TriggerKind {
    /// The `type` as written in the Definition.
    pub fn type_name(&self) -> &'static str {
        self.trigger_type().as_str()
    }

    fn trigger_type(&self) -> TriggerType {
        match self {
            Self::Cron(_) => TriggerType::Cron,
            Self::Queue(_) => TriggerType::Queue,
            Self::Webhook(_) => TriggerType::Webhook,
            Self::Manual => TriggerType::Manual,
            Self::Event(_) => TriggerType::Event,
        }
    }
}

/// `type: cron`: starts Runs on a cron schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CronTrigger {
    /// Cron expression with five fields (minute, hour, day of month, month, day of week),
    /// e.g. `0 9 * * *`. `L`, `W`, `#` and aliases such as `@daily` are accepted.
    pub expr: String,
    /// IANA time zone the expression is read in, e.g. `America/Sao_Paulo`. Defaults to UTC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    /// Delay added to each firing, up to this Duration. Each occurrence gets its own delay,
    /// the same on every scheduler.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jitter: Option<Duration>,
    /// What to do when the previous Run of this Trigger is still active. Defaults to `skip`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlap: Option<OverlapPolicy>,
    /// What to do with firings missed while bia was down. Defaults to `latest`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catch_up: Option<CatchUpPolicy>,
}

impl CronTrigger {
    pub fn cron(&self) -> Result<Cron, CronError> {
        Cron::new(&self.expr, self.tz.as_deref())
    }

    pub fn overlap_policy(&self) -> OverlapPolicy {
        self.overlap.unwrap_or(OverlapPolicy::Skip)
    }

    pub fn catch_up_policy(&self) -> CatchUpPolicy {
        self.catch_up.unwrap_or(CatchUpPolicy::Latest)
    }
}

/// What a cron Trigger does when the previous Run is still active: `skip` the firing,
/// `queue` it until that Run finishes, or `allow` it to start at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    Skip,
    Queue,
    Allow,
}

impl OverlapPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Queue => "queue",
            Self::Allow => "allow",
        }
    }
}

/// What a cron Trigger does with firings missed while bia was down: fire `none` of them,
/// only the `latest`, or `all` of them, up to the 100 most recent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatchUpPolicy {
    None,
    Latest,
    All,
}

impl CatchUpPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Latest => "latest",
            Self::All => "all",
        }
    }
}

/// `type: queue`: starts Runs from the Messages of a Queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct QueueTrigger {
    pub queue: QueueName,
    /// How many Messages one Run receives.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub batch_size: Option<u32>,
}

/// `type: webhook`: starts Runs from requests to `/hooks/{workflow_id}/{trigger_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct WebhookTrigger {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hmac: Option<Hmac>,
}

/// HMAC signature check for a webhook Trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hmac {
    /// Request header carrying the signature.
    pub header: String,
    /// Secret holding the signing key, as `secrets.NAME`.
    pub secret: String,
}

// Used only to generate the schema; its doc comment is the variant's description.
/// `type: manual`: started by hand from the API, CLI or UI.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct ManualTrigger {}

/// `type: event`: starts Runs when an Event with this name arrives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct EventTrigger {
    /// Event name.
    pub event: String,
    /// Correlation Key: when set, only Events carrying this key start a Run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#match: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TriggerType {
    Cron,
    Queue,
    Webhook,
    Manual,
    Event,
}

impl TriggerType {
    const ALL: [Self; 5] = [
        Self::Cron,
        Self::Queue,
        Self::Webhook,
        Self::Manual,
        Self::Event,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::Cron => "cron",
            Self::Queue => "queue",
            Self::Webhook => "webhook",
            Self::Manual => "manual",
            Self::Event => "event",
        }
    }
}

/// Every field any Trigger may carry. Which ones are allowed depends on `type`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTrigger {
    id: TriggerId,
    r#type: TriggerType,
    expr: Option<String>,
    tz: Option<String>,
    jitter: Option<Duration>,
    overlap: Option<OverlapPolicy>,
    catch_up: Option<CatchUpPolicy>,
    queue: Option<QueueName>,
    batch_size: Option<u32>,
    hmac: Option<Hmac>,
    event: Option<String>,
    r#match: Option<String>,
}

/// A Trigger whose fields do not fit its `type`.
#[derive(Debug)]
enum TriggerShapeError {
    NotAllowed {
        field: &'static str,
        ty: TriggerType,
    },
    Missing {
        field: &'static str,
        ty: TriggerType,
    },
}

impl fmt::Display for TriggerShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAllowed { field, ty } => {
                write!(
                    f,
                    "field `{field}` is not allowed on a `{}` Trigger",
                    ty.as_str()
                )
            }
            Self::Missing { field, ty } => {
                write!(f, "missing field `{field}` for a `{}` Trigger", ty.as_str())
            }
        }
    }
}

impl TryFrom<RawTrigger> for Trigger {
    type Error = TriggerShapeError;

    fn try_from(raw: RawTrigger) -> Result<Self, Self::Error> {
        let ty = raw.r#type;
        let present = [
            ("expr", raw.expr.is_some()),
            ("tz", raw.tz.is_some()),
            ("jitter", raw.jitter.is_some()),
            ("overlap", raw.overlap.is_some()),
            ("catch_up", raw.catch_up.is_some()),
            ("queue", raw.queue.is_some()),
            ("batch_size", raw.batch_size.is_some()),
            ("hmac", raw.hmac.is_some()),
            ("event", raw.event.is_some()),
            ("match", raw.r#match.is_some()),
        ];
        let allowed: &[&str] = match ty {
            TriggerType::Cron => &["expr", "tz", "jitter", "overlap", "catch_up"],
            TriggerType::Queue => &["queue", "batch_size"],
            TriggerType::Webhook => &["hmac"],
            TriggerType::Manual => &[],
            TriggerType::Event => &["event", "match"],
        };
        if let Some((field, _)) = present
            .into_iter()
            .find(|(field, present)| *present && !allowed.contains(field))
        {
            return Err(TriggerShapeError::NotAllowed { field, ty });
        }
        let required = |field| TriggerShapeError::Missing { field, ty };
        let kind = match ty {
            TriggerType::Cron => TriggerKind::Cron(CronTrigger {
                expr: raw.expr.ok_or_else(|| required("expr"))?,
                tz: raw.tz,
                jitter: raw.jitter,
                overlap: raw.overlap,
                catch_up: raw.catch_up,
            }),
            TriggerType::Queue => TriggerKind::Queue(QueueTrigger {
                queue: raw.queue.ok_or_else(|| required("queue"))?,
                batch_size: raw.batch_size,
            }),
            TriggerType::Webhook => TriggerKind::Webhook(WebhookTrigger { hmac: raw.hmac }),
            TriggerType::Manual => TriggerKind::Manual,
            TriggerType::Event => TriggerKind::Event(EventTrigger {
                event: raw.event.ok_or_else(|| required("event"))?,
                r#match: raw.r#match,
            }),
        };
        Ok(Trigger { id: raw.id, kind })
    }
}

impl Serialize for Trigger {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Flat<'a> {
            id: &'a TriggerId,
            r#type: &'static str,
            #[serde(flatten)]
            kind: FlatKind<'a>,
        }

        #[derive(Serialize)]
        #[serde(untagged)]
        enum FlatKind<'a> {
            Cron(&'a CronTrigger),
            Queue(&'a QueueTrigger),
            Webhook(&'a WebhookTrigger),
            Manual {},
            Event(&'a EventTrigger),
        }

        Flat {
            id: &self.id,
            r#type: self.kind.type_name(),
            kind: match &self.kind {
                TriggerKind::Cron(t) => FlatKind::Cron(t),
                TriggerKind::Queue(t) => FlatKind::Queue(t),
                TriggerKind::Webhook(t) => FlatKind::Webhook(t),
                TriggerKind::Manual => FlatKind::Manual {},
                TriggerKind::Event(t) => FlatKind::Event(t),
            },
        }
        .serialize(serializer)
    }
}

/// The fields every Trigger carries, used only to generate the schema.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct TriggerCommon {
    /// Unique within the Definition. Webhook Triggers are served at
    /// `/hooks/{workflow_id}/{trigger_id}`.
    id: TriggerId,
}

impl JsonSchema for Trigger {
    fn schema_name() -> Cow<'static, str> {
        "Trigger".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let common = TriggerCommon::json_schema(generator);
        let variants: Vec<Schema> = TriggerType::ALL
            .into_iter()
            .map(|ty| {
                let fields = match ty {
                    TriggerType::Cron => CronTrigger::json_schema(generator),
                    TriggerType::Queue => QueueTrigger::json_schema(generator),
                    TriggerType::Webhook => WebhookTrigger::json_schema(generator),
                    TriggerType::Manual => ManualTrigger::json_schema(generator),
                    TriggerType::Event => EventTrigger::json_schema(generator),
                };
                tagged_object(
                    &common,
                    "type",
                    json_schema!({ "const": ty.as_str() }),
                    fields,
                )
            })
            .collect();
        json_schema!({
            "description": "What starts a Run. The Trigger payload becomes `input` and its metadata becomes `trigger`.",
            "oneOf": variants,
        })
    }
}
