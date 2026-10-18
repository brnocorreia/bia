//! The `Action` trait, its descriptor and classified error, and the Action Registry interface.
//!
//! Everything an Action receives and returns serialises, so any Action can later run behind an
//! Action Server, in WASM or in an isolated Worker pool without Definitions changing (ADR-0006,
//! ADR-0027). The cancellation token is the one value that does not: it stands for the cancel
//! request a transport would carry.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dsl::{Params, StepId, WorkflowId};

mod descriptor;
mod error;
mod registry;
#[cfg(feature = "testing")]
pub mod testing;

pub use descriptor::{ActionDescriptor, ActionExample, Effect};
pub use error::{ActionError, ErrorClass};
pub use registry::{ActionRegistry, CatalogueEntry, DuplicateAction, InMemoryRegistry, Lookup};
pub use tokio_util::sync::CancellationToken;

/// The value a Step produces once its Action succeeds.
pub type Output = Value;

/// The future an Action's `execute` returns.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The kind of work a Step performs.
///
/// Timeouts and retries are the interpreter's job: an Action runs one Attempt and reports how it
/// went. It watches `cancel` and returns promptly once it fires; the interpreter knows why it
/// cancelled, so the error returned then is not what decides the Step's outcome.
pub trait Action: Send + Sync {
    /// What the Action is called, takes and returns. It does not change while bia runs.
    fn descriptor(&self) -> &ActionDescriptor;

    /// Runs one Attempt.
    fn execute(
        &self,
        call: ActionCall,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>>;
}

impl fmt::Debug for dyn Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Action")
            .field("name", &self.descriptor().name)
            .finish_non_exhaustive()
    }
}

/// Everything one Attempt of an Action receives besides the cancellation token.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionCall {
    /// The Step's Params with Templates and Secrets resolved.
    pub params: Params,
    /// Forwarded to external systems that deduplicate requests.
    pub idempotency_key: IdempotencyKey,
    /// Where the Attempt belongs, for logging correlation.
    pub context: RunContext,
}

// Resolved Params may hold Secrets, so only their names are printed.
impl fmt::Debug for ActionCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActionCall")
            .field("params", &self.params.keys().collect::<Vec<_>>())
            .field("idempotency_key", &self.idempotency_key)
            .field("context", &self.context)
            .finish()
    }
}

/// A key stable across a Step's Attempts, `run_id:step_id:iteration_path` (ADR-0010).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The Run an Attempt belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunContext {
    pub run_id: String,
    pub workflow_id: WorkflowId,
    /// The Version the Run is pinned to.
    pub version: u32,
    pub step_id: StepId,
    /// Starts at 1.
    pub attempt: u32,
}
