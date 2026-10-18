//! A configurable fake Action for tests in any crate, behind the `testing` feature.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use serde_json::Value;

use super::{
    Action, ActionCall, ActionDescriptor, ActionError, BoxFuture, CancellationToken, Effect, Output,
};
use crate::dsl::ActionName;

/// An Action that records its calls and behaves as configured. By default it is pure and
/// returns its Params as its Output.
#[derive(Debug)]
pub struct FakeAction {
    descriptor: ActionDescriptor,
    behaviour: Behaviour,
    calls: Mutex<Vec<ActionCall>>,
    saw_cancellation: AtomicBool,
}

#[derive(Debug)]
enum Behaviour {
    EchoParams,
    Return(Output),
    Fail(ActionError),
    WaitForCancellation,
}

impl FakeAction {
    /// A pure Action named `name`, which must be a valid `category.operation`.
    ///
    /// # Panics
    ///
    /// When `name` is not a valid Action name.
    pub fn new(name: &str) -> Self {
        let name = ActionName::new(name).unwrap_or_else(|e| panic!("{e}"));
        Self::from_descriptor(ActionDescriptor::new(
            name,
            "A fake Action for tests.",
            Effect::Pure,
        ))
    }

    pub fn from_descriptor(descriptor: ActionDescriptor) -> Self {
        Self {
            descriptor,
            behaviour: Behaviour::EchoParams,
            calls: Mutex::new(Vec::new()),
            saw_cancellation: AtomicBool::new(false),
        }
    }

    /// Succeeds with `output` on every call.
    pub fn returning(mut self, output: Output) -> Self {
        self.behaviour = Behaviour::Return(output);
        self
    }

    /// Fails with `error` on every call.
    pub fn failing(mut self, error: ActionError) -> Self {
        self.behaviour = Behaviour::Fail(error);
        self
    }

    /// Runs until the cancellation token fires, then fails with a `retryable` error.
    pub fn until_cancelled(mut self) -> Self {
        self.behaviour = Behaviour::WaitForCancellation;
        self
    }

    /// Changes the descriptor, e.g. to mark the Action dangerous or side-effecting.
    pub fn with_descriptor(mut self, change: impl FnOnce(&mut ActionDescriptor)) -> Self {
        change(&mut self.descriptor);
        self
    }

    /// Every call received so far, oldest first.
    pub fn calls(&self) -> Vec<ActionCall> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether a running call observed its cancellation token fire.
    pub fn saw_cancellation(&self) -> bool {
        self.saw_cancellation.load(Ordering::SeqCst)
    }
}

impl Action for FakeAction {
    fn descriptor(&self) -> &ActionDescriptor {
        &self.descriptor
    }

    fn execute(
        &self,
        call: ActionCall,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        Box::pin(async move {
            let params = call.params.clone();
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call);
            match &self.behaviour {
                Behaviour::EchoParams => Ok(Value::Object(params)),
                Behaviour::Return(output) => Ok(output.clone()),
                Behaviour::Fail(error) => Err(error.clone()),
                Behaviour::WaitForCancellation => {
                    cancel.cancelled().await;
                    self.saw_cancellation.store(true, Ordering::SeqCst);
                    Err(ActionError::retryable("cancelled"))
                }
            }
        })
    }
}
