//! Steps and the Blocks that nest them.
//!
//! In a Definition a Step is one flat object: the common fields, `action`, and then either
//! `params` (ordinary Actions) or the nested fields of a Block (`flow.branch`, `flow.switch`,
//! …). Deserialisation reads every field a Step may carry in one pass, so errors keep their
//! JSON path, and then checks the fields against the Action (ADR-0024).

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use super::schema::tagged_object;
use super::{ActionName, Duration, Expression, StepId};

/// The input a Step passes to its Action. Values may contain Templates.
pub type Params = serde_json::Map<String, Value>;

/// One node of a Definition.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "RawStep")]
pub struct Step {
    pub id: StepId,
    pub description: Option<String>,
    /// CEL condition; when false the Step is recorded as `skipped`.
    pub r#if: Option<Expression>,
    pub retry: Option<RetryPolicy>,
    /// Per Attempt.
    pub timeout: Option<Duration>,
    pub kind: StepKind,
}

/// What a Step does: call an ordinary Action, or run a Block.
#[derive(Debug, Clone, PartialEq)]
pub enum StepKind {
    Action { action: ActionName, params: Params },
    Branch(Branch),
    Switch(Switch),
    Parallel(Parallel),
    Foreach(Foreach),
    LoopUntil(LoopUntil),
    Try(Try),
}

impl Step {
    /// The Action name as written in the Definition.
    pub fn action(&self) -> &str {
        self.kind.action()
    }
}

impl StepKind {
    /// The Action name as written in the Definition.
    pub fn action(&self) -> &str {
        match self {
            Self::Action { action, .. } => action.as_str(),
            Self::Branch(_) => Branch::ACTION,
            Self::Switch(_) => Switch::ACTION,
            Self::Parallel(_) => Parallel::ACTION,
            Self::Foreach(_) => Foreach::ACTION,
            Self::LoopUntil(_) => LoopUntil::ACTION,
            Self::Try(_) => Try::ACTION,
        }
    }

    /// Whether this is a Block, i.e. holds nested Step lists.
    pub fn is_block(&self) -> bool {
        !matches!(self, Self::Action { .. })
    }
}

/// Retry policy for `retryable` errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// Total Attempts, the first one included.
    #[schemars(range(min = 1))]
    pub max_attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff: Option<Backoff>,
    /// Delay before the first retry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial: Option<Duration>,
    /// Upper bound on the delay between Attempts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<Duration>,
}

/// How the delay grows between Attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Backoff {
    Fixed,
    Exponential,
}

/// `flow.branch`: runs `then` when `when` is true, `else` otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Branch {
    /// CEL condition.
    pub when: Expression,
    pub then: Vec<Step>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub r#else: Vec<Step>,
}

impl Branch {
    pub const ACTION: &str = "flow.branch";
}

/// `flow.switch`: runs the Steps of the first case whose `equals` matches `value`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Switch {
    /// CEL Expression compared with each case's `equals`.
    pub value: Expression,
    pub cases: Vec<SwitchCase>,
    /// Steps run when no case matches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default: Vec<Step>,
}

impl Switch {
    pub const ACTION: &str = "flow.switch";
}

/// One case of a `flow.switch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwitchCase {
    /// The value `value` must equal for this case to run.
    pub equals: Value,
    pub steps: Vec<Step>,
}

/// `flow.parallel`: runs each branch concurrently. Its Output is the list of branch Outputs.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Parallel {
    pub branches: Vec<Vec<Step>>,
    /// When the Block completes. Defaults to `all`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join: Option<Join>,
    /// How many branches must succeed for `join: n_of_m`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<FailureMode>,
}

impl Parallel {
    pub const ACTION: &str = "flow.parallel";
}

/// `flow.foreach`: runs `body` once per item, with `item` and `index` in scope. Its Output is
/// the list of each iteration's last Output.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Foreach {
    /// CEL Expression producing the list to iterate.
    pub items: Expression,
    pub body: Vec<Step>,
    /// How many iterations run at once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<FailureMode>,
}

impl Foreach {
    pub const ACTION: &str = "flow.foreach";
}

/// `flow.loop_until`: runs `body` until `until` is true.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct LoopUntil {
    pub body: Vec<Step>,
    /// CEL condition evaluated after each iteration; it sees that iteration's Outputs.
    pub until: Expression,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
}

impl LoopUntil {
    pub const ACTION: &str = "flow.loop_until";
}

/// `flow.try`: runs `body`, and `catch` if it fails, with `error` in scope.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Try {
    pub body: Vec<Step>,
    pub catch: Vec<Step>,
}

impl Try {
    pub const ACTION: &str = "flow.try";
}

/// When a `flow.parallel` Block completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Join {
    All,
    Any,
    NOfM,
}

/// What a failing branch or iteration does to its siblings. Defaults to `fail_fast`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    FailFast,
    AllSettled,
}

/// Every Block Action, in the order the schema lists them.
pub(crate) const BLOCK_ACTIONS: [&str; 6] = [
    Branch::ACTION,
    Switch::ACTION,
    Parallel::ACTION,
    Foreach::ACTION,
    LoopUntil::ACTION,
    Try::ACTION,
];

/// Every field any Step may carry. Which ones are allowed depends on `action`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    id: StepId,
    action: ActionName,
    description: Option<String>,
    r#if: Option<Expression>,
    retry: Option<RetryPolicy>,
    timeout: Option<Duration>,
    params: Option<Params>,
    when: Option<Expression>,
    then: Option<Vec<Step>>,
    r#else: Option<Vec<Step>>,
    value: Option<Expression>,
    cases: Option<Vec<SwitchCase>>,
    default: Option<Vec<Step>>,
    branches: Option<Vec<Vec<Step>>>,
    join: Option<Join>,
    n: Option<u32>,
    mode: Option<FailureMode>,
    items: Option<Expression>,
    body: Option<Vec<Step>>,
    concurrency: Option<u32>,
    max_iterations: Option<u32>,
    until: Option<Expression>,
    catch: Option<Vec<Step>>,
}

/// A Step whose fields do not fit its Action.
#[derive(Debug)]
enum StepShapeError {
    NotAllowed { field: &'static str, action: String },
    Missing { field: &'static str, action: String },
}

impl fmt::Display for StepShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAllowed { field, action } => {
                write!(f, "field `{field}` is not allowed on a `{action}` Step")
            }
            Self::Missing { field, action } => {
                write!(f, "missing field `{field}` for a `{action}` Step")
            }
        }
    }
}

impl RawStep {
    /// Names of the action-specific fields that are present.
    fn present_fields(&self) -> Vec<&'static str> {
        [
            ("params", self.params.is_some()),
            ("when", self.when.is_some()),
            ("then", self.then.is_some()),
            ("else", self.r#else.is_some()),
            ("value", self.value.is_some()),
            ("cases", self.cases.is_some()),
            ("default", self.default.is_some()),
            ("branches", self.branches.is_some()),
            ("join", self.join.is_some()),
            ("n", self.n.is_some()),
            ("mode", self.mode.is_some()),
            ("items", self.items.is_some()),
            ("body", self.body.is_some()),
            ("concurrency", self.concurrency.is_some()),
            ("max_iterations", self.max_iterations.is_some()),
            ("until", self.until.is_some()),
            ("catch", self.catch.is_some()),
        ]
        .into_iter()
        .filter_map(|(field, present)| present.then_some(field))
        .collect()
    }
}

impl TryFrom<RawStep> for Step {
    type Error = StepShapeError;

    fn try_from(raw: RawStep) -> Result<Self, Self::Error> {
        let action = raw.action.as_str().to_owned();
        let allowed: &[&str] = match action.as_str() {
            Branch::ACTION => &["when", "then", "else"],
            Switch::ACTION => &["value", "cases", "default"],
            Parallel::ACTION => &["branches", "join", "n", "mode"],
            Foreach::ACTION => &["items", "body", "concurrency", "max_iterations", "mode"],
            LoopUntil::ACTION => &["body", "until", "max_iterations"],
            Try::ACTION => &["body", "catch"],
            _ => &["params"],
        };
        if let Some(field) = raw
            .present_fields()
            .into_iter()
            .find(|f| !allowed.contains(f))
        {
            return Err(StepShapeError::NotAllowed { field, action });
        }
        let required = |field: &'static str| StepShapeError::Missing {
            field,
            action: action.clone(),
        };
        let kind = match action.as_str() {
            Branch::ACTION => StepKind::Branch(Branch {
                when: raw.when.ok_or_else(|| required("when"))?,
                then: raw.then.ok_or_else(|| required("then"))?,
                r#else: raw.r#else.unwrap_or_default(),
            }),
            Switch::ACTION => StepKind::Switch(Switch {
                value: raw.value.ok_or_else(|| required("value"))?,
                cases: raw.cases.ok_or_else(|| required("cases"))?,
                default: raw.default.unwrap_or_default(),
            }),
            Parallel::ACTION => StepKind::Parallel(Parallel {
                branches: raw.branches.ok_or_else(|| required("branches"))?,
                join: raw.join,
                n: raw.n,
                mode: raw.mode,
            }),
            Foreach::ACTION => StepKind::Foreach(Foreach {
                items: raw.items.ok_or_else(|| required("items"))?,
                body: raw.body.ok_or_else(|| required("body"))?,
                concurrency: raw.concurrency,
                max_iterations: raw.max_iterations,
                mode: raw.mode,
            }),
            LoopUntil::ACTION => StepKind::LoopUntil(LoopUntil {
                body: raw.body.ok_or_else(|| required("body"))?,
                until: raw.until.ok_or_else(|| required("until"))?,
                max_iterations: raw.max_iterations,
            }),
            Try::ACTION => StepKind::Try(Try {
                body: raw.body.ok_or_else(|| required("body"))?,
                catch: raw.catch.ok_or_else(|| required("catch"))?,
            }),
            _ => StepKind::Action {
                action: raw.action,
                params: raw.params.unwrap_or_default(),
            },
        };
        Ok(Step {
            id: raw.id,
            description: raw.description,
            r#if: raw.r#if,
            retry: raw.retry,
            timeout: raw.timeout,
            kind,
        })
    }
}

impl Serialize for Step {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Flat<'a> {
            id: &'a StepId,
            action: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            description: Option<&'a String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            r#if: Option<&'a Expression>,
            #[serde(skip_serializing_if = "Option::is_none")]
            retry: Option<&'a RetryPolicy>,
            #[serde(skip_serializing_if = "Option::is_none")]
            timeout: Option<&'a Duration>,
            #[serde(flatten)]
            kind: FlatKind<'a>,
        }

        #[derive(Serialize)]
        #[serde(untagged)]
        enum FlatKind<'a> {
            Action {
                #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
                params: &'a Params,
            },
            Branch(&'a Branch),
            Switch(&'a Switch),
            Parallel(&'a Parallel),
            Foreach(&'a Foreach),
            LoopUntil(&'a LoopUntil),
            Try(&'a Try),
        }

        Flat {
            id: &self.id,
            action: self.action(),
            description: self.description.as_ref(),
            r#if: self.r#if.as_ref(),
            retry: self.retry.as_ref(),
            timeout: self.timeout.as_ref(),
            kind: match &self.kind {
                StepKind::Action { params, .. } => FlatKind::Action { params },
                StepKind::Branch(b) => FlatKind::Branch(b),
                StepKind::Switch(b) => FlatKind::Switch(b),
                StepKind::Parallel(b) => FlatKind::Parallel(b),
                StepKind::Foreach(b) => FlatKind::Foreach(b),
                StepKind::LoopUntil(b) => FlatKind::LoopUntil(b),
                StepKind::Try(b) => FlatKind::Try(b),
            },
        }
        .serialize(serializer)
    }
}

/// The fields every Step carries, used only to generate the schema.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct StepCommon {
    /// Unique across the whole Definition, nested Blocks included.
    id: StepId,
    /// Free text shown in the UI and the Timeline.
    description: Option<String>,
    /// CEL condition. When false, the Step is recorded as `skipped`.
    r#if: Option<Expression>,
    /// Applies to `retryable` errors only.
    retry: Option<RetryPolicy>,
    /// Timeout per Attempt.
    timeout: Option<Duration>,
}

// Used only to generate the schema; its doc comment is the variant's description.
/// A Step that calls an ordinary Action with `params`.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct ActionFields {
    /// Validated against the Action's param schema after Templates are resolved.
    params: Option<Params>,
}

impl JsonSchema for Step {
    fn schema_name() -> Cow<'static, str> {
        "Step".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let common = StepCommon::json_schema(generator);
        let action = {
            let mut name = generator.subschema_for::<ActionName>();
            name.insert("not".into(), serde_json::json!({ "enum": BLOCK_ACTIONS }));
            name
        };
        let variants = [
            tagged_object(
                &common,
                "action",
                action,
                ActionFields::json_schema(generator),
            ),
            block(generator, &common, Branch::ACTION, Branch::json_schema),
            block(generator, &common, Switch::ACTION, Switch::json_schema),
            block(generator, &common, Parallel::ACTION, Parallel::json_schema),
            block(generator, &common, Foreach::ACTION, Foreach::json_schema),
            block(
                generator,
                &common,
                LoopUntil::ACTION,
                LoopUntil::json_schema,
            ),
            block(generator, &common, Try::ACTION, Try::json_schema),
        ];
        json_schema!({
            "description": "One node of a Definition. Ordinary Actions take `params`; Blocks (`flow.branch`, `flow.switch`, `flow.parallel`, `flow.foreach`, `flow.loop_until`, `flow.try`) take nested Step lists instead.",
            "oneOf": variants,
        })
    }
}

fn block(
    generator: &mut SchemaGenerator,
    common: &Schema,
    action: &str,
    fields: fn(&mut SchemaGenerator) -> Schema,
) -> Schema {
    tagged_object(
        common,
        "action",
        json_schema!({ "const": action }),
        fields(generator),
    )
}
