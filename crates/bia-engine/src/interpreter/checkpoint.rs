//! What the interpreter keeps in a Run's Checkpoint: the Cursor and the Run state (ADR-0032).

use std::collections::BTreeMap;

use bia_core::dsl::{Definition, Step};
use bia_store::run::{Checkpoint, RunId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::EngineError;

/// The Step list a Definition runs from the top.
const STEPS: &str = "steps";

/// Where a Run is inside its Definition: the JSON Pointer path to the Step it executes next,
/// stored as segments, e.g. `["steps", 2]` for the third top-level Step.
///
/// Blocks extend the path with the nested list and index, e.g. `["steps", 2, "then", 0]`;
/// until they run (BIA-14), a Cursor is one top-level position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cursor {
    index: usize,
}

impl Cursor {
    /// The first top-level Step.
    pub(crate) fn start() -> Self {
        Self { index: 0 }
    }

    pub(crate) fn decode(run: RunId, segments: &[Value]) -> Result<Self, EngineError> {
        match segments {
            [Value::String(list), Value::Number(index)] if list == STEPS => index
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .map(|index| Self { index })
                .ok_or_else(|| invalid(run, format!("cursor index {index} is not a position"))),
            other => Err(invalid(
                run,
                format!("cursor {} names no Step this build can run", json!(other)),
            )),
        }
    }

    pub(crate) fn encode(self) -> Vec<Value> {
        vec![json!(STEPS), json!(self.index)]
    }

    /// The Step at the Cursor, or `None` once the list it points into is finished.
    pub(crate) fn step(self, definition: &Definition) -> Option<&Step> {
        definition.steps.get(self.index)
    }

    /// The position after this one in the same list.
    pub(crate) fn next(self) -> Self {
        Self {
            index: self.index + 1,
        }
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
}

impl RunState {
    pub(crate) fn decode(run: RunId, state: &Value) -> Result<Self, EngineError> {
        // A pending Run's state is `{}`, which decodes to the default.
        Self::deserialize(state).map_err(|error| invalid(run, error))
    }
}

/// The Checkpoint of a Run at `cursor` with `state`.
pub(crate) fn checkpoint(cursor: Option<Cursor>, state: &RunState) -> Checkpoint {
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
