//! Interpreter, Worker loop, scheduler loop and Trigger dispatch.
//!
//! The Action Registry is injected; this crate does not depend on `bia-actions`.

mod error;
pub mod interpreter;
mod secrets;
mod worker;

pub use error::EngineError;
pub use interpreter::{
    Advanced, Interpreter, InterpreterConfig, Limit, LimitExceeded, RunLimits, cancel_run,
    start_run,
};
pub use secrets::{NoSecrets, SecretResolver};
pub use worker::{Worker, WorkerConfig};
