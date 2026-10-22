//! Where the interpreter resolves `secrets.NAME` at execution time.

use std::fmt;

use bia_core::action::{ActionError, BoxFuture};

/// Resolves the Secrets a Step's Params read, just before its Action runs. Secret Providers
/// implement it (ADR-0017). Resolved values reach only the Action call, never the Checkpoint,
/// the Timeline or errors.
pub trait SecretResolver: fmt::Debug + Send + Sync {
    /// The value of Secret `name`, or `None` when no Secret has that name. An error is
    /// recorded on the Attempt, so its message must not contain a Secret value.
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<String>, ActionError>>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn resolve<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Result<Option<String>, ActionError>> {
        Box::pin(async { Ok(None) })
    }
}
