//! Interpreter, Worker loop, scheduler loop and Trigger dispatch.
//!
//! The Action Registry is injected; this crate does not depend on `bia-actions`.

mod error;
pub mod interpreter;
mod secrets;

pub use error::EngineError;
pub use interpreter::{Advanced, Interpreter, InterpreterConfig, start_run};
pub use secrets::{NoSecrets, SecretResolver};
