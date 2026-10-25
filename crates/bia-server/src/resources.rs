//! Where validation looks up the Queues a Definition names.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

/// The Queues that exist. Pluggable because Queue resources arrive after Workflows: until a
/// directory is supplied, no Queue exists and a Definition naming one cannot be published.
pub trait QueueDirectory: Send + Sync {
    fn names(&self) -> Pin<Box<dyn Future<Output = BTreeSet<String>> + Send + '_>>;
}

/// A fixed set of Queue names.
#[derive(Debug, Clone, Default)]
pub struct QueueNames(pub BTreeSet<String>);

impl QueueDirectory for QueueNames {
    fn names(&self) -> Pin<Box<dyn Future<Output = BTreeSet<String>> + Send + '_>> {
        Box::pin(async { self.0.clone() })
    }
}
