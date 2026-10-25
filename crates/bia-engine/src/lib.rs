//! Interpreter, Worker loop, scheduler loop and Trigger dispatch.
//!
//! The Action Registry is injected; this crate does not depend on `bia-actions`.

mod error;
pub mod interpreter;
mod retention;
mod scheduler;
mod secrets;
mod worker;

pub use error::{EngineError, NotResumable};
pub use interpreter::{
    Advanced, DryRun, DryRunResult, Interpreter, InterpreterConfig, Limit, LimitExceeded, Mock,
    MockOutcome, Mocks, RunLimits, cancel_run, resume_run, start_run,
};
pub use retention::{Retention, RetentionConfig};
pub use scheduler::{CATCH_UP_LIMIT, Chore, MISSED_AFTER, QUEUE_LIMIT, Scheduler, SchedulerConfig};
pub use secrets::{NoSecrets, SecretResolver};
pub use worker::{Worker, WorkerConfig};
