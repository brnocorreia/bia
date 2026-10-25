//! Where the interpreter resolves `secrets.NAME` at execution time.

use std::fmt;

use bia_core::action::{ActionError, BoxFuture};
use bia_store::secret::{MasterKey, PgSecretRepository, SecretName, SecretRepository};
use serde_json::Value;
use sqlx::PgPool;

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

/// The internal Secret Provider: Secrets in Postgres, decrypted under the master key
/// (ADR-0017, ADR-0049).
#[derive(Debug, Clone)]
pub struct InternalSecrets {
    pool: PgPool,
    key: Option<MasterKey>,
}

impl InternalSecrets {
    /// Without a `key`, resolving any Secret fails `fatal`, naming `BIA_MASTER_KEY`.
    pub fn new(pool: PgPool, key: Option<MasterKey>) -> Self {
        Self { pool, key }
    }
}

impl SecretResolver for InternalSecrets {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<String>, ActionError>> {
        Box::pin(async move {
            let Some(key) = &self.key else {
                return Err(ActionError::fatal(format!(
                    "Secret '{name}' cannot be read: the server has no BIA_MASTER_KEY"
                )));
            };
            // A name that cannot be stored cannot exist.
            let Ok(secret) = SecretName::new(name) else {
                return Ok(None);
            };
            let sealed = async {
                let mut conn = self.pool.acquire().await?;
                PgSecretRepository.get(&mut conn, &secret).await
            }
            .await
            .map_err(|error| {
                ActionError::retryable(format!("cannot read Secret '{name}': {error}"))
            })?;
            sealed
                .map(|sealed| key.open(&secret, &sealed))
                .transpose()
                .map_err(|error| ActionError::fatal(format!("Secret '{name}': {error}")))
        })
    }
}

/// Replaces every resolved Secret value with [`MASK`] in what an Attempt hands back, so an
/// Action that echoes its Params into an Output or an error does not write a Secret to the
/// Timeline (ADR-0049). Matching is on the exact value, so a transformed copy (encoded, split,
/// cased) is not caught.
#[derive(Default)]
pub(crate) struct Masker {
    values: Vec<String>,
}

pub(crate) const MASK: &str = "***";

impl Masker {
    pub(crate) fn add(&mut self, value: &str) {
        if !value.is_empty() {
            self.values.push(value.to_owned());
            // Longest first, so a value containing another is masked whole.
            self.values
                .sort_by_key(|value| std::cmp::Reverse(value.len()));
        }
    }

    fn text(&self, text: &str) -> String {
        self.values.iter().fold(text.to_owned(), |text, value| {
            text.replace(value.as_str(), MASK)
        })
    }

    pub(crate) fn json(&self, value: Value) -> Value {
        if self.values.is_empty() {
            return value;
        }
        match value {
            Value::String(text) => Value::String(self.text(&text)),
            Value::Array(items) => Value::Array(items.into_iter().map(|v| self.json(v)).collect()),
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(key, v)| (self.text(&key), self.json(v)))
                    .collect(),
            ),
            other => other,
        }
    }

    pub(crate) fn error(&self, error: ActionError) -> ActionError {
        if self.values.is_empty() {
            return error;
        }
        let masked = ActionError::new(error.class(), self.text(error.message()));
        match error.details() {
            Some(details) => masked.with_details(self.json(details.clone())),
            None => masked,
        }
    }
}

impl fmt::Debug for Masker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Masker").finish_non_exhaustive()
    }
}
