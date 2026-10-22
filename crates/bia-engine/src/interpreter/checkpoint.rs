//! What the interpreter keeps in a Run's Checkpoint: the Cursor and the Run state (ADR-0032).

use std::collections::BTreeMap;

use bia_core::dsl::{Definition, Step, StepKind};
use bia_store::run::{Checkpoint, RunId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::EngineError;

const STEPS: &str = "steps";
/// The nested lists of `flow.branch`.
pub(crate) const THEN: &str = "then";
pub(crate) const ELSE: &str = "else";

/// Where a Run is inside its Definition: the JSON Pointer path to the Step it executes next,
/// stored as segments, one `[list, index]` pair per level of Block, e.g. `["steps", 2]` for the
/// third top-level Step and `["steps", 2, "then", 0]` for the first Step of that Block's
/// `then` (ADR-0003, ADR-0035).
///
/// A Cursor always rests on a Step that exists. Finishing a Step moves it to the next Step of
/// its list; when the list is used up it leaves the Block and moves on in the parent list, so
/// every Block gets the same generic enter-and-return behaviour from [`Cursor::enter`] and
/// [`Cursor::after`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cursor {
    frames: Vec<Frame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Frame {
    list: String,
    index: usize,
}

impl Cursor {
    /// The first top-level Step, if the Definition has one.
    pub(crate) fn start(definition: &Definition) -> Option<Self> {
        Self {
            frames: vec![Frame {
                list: STEPS.to_owned(),
                index: 0,
            }],
        }
        .settle(definition)
    }

    /// Reads a stored Cursor, checking that it names a Step of `definition`.
    pub(crate) fn decode(
        run: RunId,
        segments: &[Value],
        definition: &Definition,
    ) -> Result<Self, EngineError> {
        let bad = || invalid(run, format!("cursor {} names no Step", json!(segments)));
        if segments.is_empty() || !segments.len().is_multiple_of(2) {
            return Err(bad());
        }
        let frames = segments
            .chunks(2)
            .map(|pair| match pair {
                [Value::String(list), Value::Number(index)] => index
                    .as_u64()
                    .and_then(|index| usize::try_from(index).ok())
                    .map(|index| Frame {
                        list: list.clone(),
                        index,
                    }),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(bad)?;
        let cursor = Self { frames };
        match cursor.step(definition) {
            Some(_) => Ok(cursor),
            None => Err(bad()),
        }
    }

    pub(crate) fn encode(&self) -> Vec<Value> {
        self.frames
            .iter()
            .flat_map(|frame| [json!(frame.list), json!(frame.index)])
            .collect()
    }

    pub(crate) fn step<'a>(&self, definition: &'a Definition) -> Option<&'a Step> {
        self.list(definition)?.get(self.frames.last()?.index)
    }

    /// The Step list the Cursor's last level points into.
    fn list<'a>(&self, definition: &'a Definition) -> Option<&'a [Step]> {
        let mut frames = self.frames.iter();
        let mut parent = frames.next().filter(|frame| frame.list == STEPS)?;
        let mut list: &[Step] = &definition.steps;
        for frame in frames {
            list = nested(list.get(parent.index)?, &frame.list)?;
            parent = frame;
        }
        Some(list)
    }

    /// The Cursor at the first Step of `list`, nested in the Block at this Cursor, or `None`
    /// when `list` is empty and the Run carries on after the Block.
    pub(crate) fn enter(mut self, definition: &Definition, list: &str) -> Option<Self> {
        self.frames.push(Frame {
            list: list.to_owned(),
            index: 0,
        });
        self.settle(definition)
    }

    /// The Cursor once the Step at this one has finished: the next Step of its list, or, past
    /// the end of a Block's list, the Step after that Block. `None` when the Run has no Step
    /// left.
    pub(crate) fn after(mut self, definition: &Definition) -> Option<Self> {
        self.frames.last_mut()?.index += 1;
        self.settle(definition)
    }

    /// Leaves every list the Cursor has run off the end of, so it rests on a Step or on nothing.
    fn settle(mut self, definition: &Definition) -> Option<Self> {
        while self.step(definition).is_none() {
            // Past the end of a list: the Block that holds it is done, and the Cursor moves on
            // in the list around that Block. A used-up top-level list ends the Run.
            self.frames.pop();
            self.frames.last_mut()?.index += 1;
        }
        Some(self)
    }
}

/// The nested Step list a Block calls `list`.
fn nested<'a>(step: &'a Step, list: &str) -> Option<&'a [Step]> {
    match (&step.kind, list) {
        (StepKind::Branch(branch), THEN) => Some(&branch.then),
        (StepKind::Branch(branch), ELSE) => Some(&branch.r#else),
        _ => None,
    }
}

/// Everything besides the Cursor a Run needs to resume.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RunState {
    /// The Output of every Step that succeeded, by Step id, readable as `steps.<id>.output`.
    #[serde(default)]
    pub(crate) outputs: BTreeMap<String, Value>,
    /// Attempts the Step at the Cursor has finished so far; the next one is `attempts + 1`.
    #[serde(default)]
    pub(crate) attempts: u32,
    /// Set while the Step at the Cursor is a `flow.sleep` or `flow.sleep_until` the Run is
    /// waiting out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sleep: Option<Sleep>,
}

/// A sleep in progress. Times are milliseconds since the Unix epoch, so the Checkpoint needs no
/// date format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Sleep {
    started_at: i64,
    until: i64,
    /// The resolved Params of the sleeping Step, for the Timeline entry written on waking.
    input: Option<Value>,
}

impl Sleep {
    pub(crate) fn new(
        started_at: DateTime<Utc>,
        until: DateTime<Utc>,
        input: Option<Value>,
    ) -> Self {
        Self {
            started_at: started_at.timestamp_millis(),
            until: until.timestamp_millis(),
            input,
        }
    }

    pub(crate) fn started_at(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.started_at).unwrap_or_default()
    }

    pub(crate) fn until(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.until).unwrap_or_default()
    }

    pub(crate) fn input(&self) -> Option<&Value> {
        self.input.as_ref()
    }
}

impl RunState {
    pub(crate) fn decode(run: RunId, state: &Value) -> Result<Self, EngineError> {
        // A pending Run's state is `{}`, which decodes to the default.
        Self::deserialize(state).map_err(|error| invalid(run, error))
    }
}

pub(crate) fn checkpoint(cursor: Option<&Cursor>, state: &RunState) -> Checkpoint {
    Checkpoint {
        cursor: cursor.map(Cursor::encode).unwrap_or_default(),
        state: serde_json::to_value(state).unwrap_or_else(|_| json!({})),
    }
}

fn invalid(run: RunId, message: impl ToString) -> EngineError {
    EngineError::InvalidCheckpoint {
        run,
        message: message.to_string(),
    }
}
