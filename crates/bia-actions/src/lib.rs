//! Builtin Actions, one module per category.
//!
//! Depends only on `bia-core`.

use std::sync::Arc;

use bia_core::action::{Action, InMemoryRegistry};

pub mod http;
mod params;
pub mod transform;

/// Something that stops the builtin Actions from being set up.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuiltinError {
    /// The HTTP client could not be built, e.g. because the system trust store is unreadable.
    #[error("cannot build the HTTP client: {0}")]
    HttpClient(#[source] reqwest::Error),
}

/// A registry holding every builtin Action, for `bia-cli` to inject into the engine.
pub fn builtin_registry() -> Result<InMemoryRegistry, BuiltinError> {
    let mut registry = InMemoryRegistry::new();
    let actions: [Arc<dyn Action>; 2] = [
        Arc::new(http::Request::new()?),
        Arc::new(transform::Expr::new()),
    ];
    let flow = bia_core::action::flow::Flow::all()
        .into_iter()
        .map(|action| Arc::new(action) as Arc<dyn Action>);
    for action in actions.into_iter().chain(flow) {
        registry
            .register(action)
            .expect("builtin Actions have distinct names");
    }
    Ok(registry)
}
