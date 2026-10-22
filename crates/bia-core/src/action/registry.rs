//! The Action Registry: the Actions a running bia knows about, and which of them are enabled.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use serde::Serialize;

use super::{Action, ActionDescriptor};
use crate::dsl::ActionName;

/// What the engine, validation and the API query for Actions. The engine receives it by
/// injection, so builtin Actions and Action Servers can sit behind the same interface.
pub trait ActionRegistry: Send + Sync {
    fn lookup(&self, name: &str) -> Lookup;

    /// Every Action known, disabled ones included, ordered by name.
    fn catalogue(&self) -> Vec<CatalogueEntry>;
}

#[derive(Debug, Clone)]
pub enum Lookup {
    Enabled(Arc<dyn Action>),
    /// Known but turned off, e.g. a dangerous Action not enabled in server config (ADR-0014).
    /// Only its descriptor is handed out, so it cannot be executed.
    Disabled(ActionDescriptor),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CatalogueEntry {
    #[serde(flatten)]
    pub descriptor: ActionDescriptor,
    pub enabled: bool,
}

#[derive(Default)]
pub struct InMemoryRegistry {
    actions: BTreeMap<ActionName, Arc<dyn Action>>,
    enabled_dangerous: BTreeSet<ActionName>,
}

impl InMemoryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, action: Arc<dyn Action>) -> Result<(), DuplicateAction> {
        let name = action.descriptor().name.clone();
        if self.actions.contains_key(&name) {
            return Err(DuplicateAction { name });
        }
        self.actions.insert(name, action);
        Ok(())
    }

    /// Enables a dangerous Action. Actions that are not dangerous are always enabled.
    pub fn enable(&mut self, name: ActionName) {
        self.enabled_dangerous.insert(name);
    }

    fn is_enabled(&self, action: &dyn Action) -> bool {
        let descriptor = action.descriptor();
        !descriptor.dangerous || self.enabled_dangerous.contains(&descriptor.name)
    }
}

impl ActionRegistry for InMemoryRegistry {
    fn lookup(&self, name: &str) -> Lookup {
        match self.actions.get(name) {
            Some(action) if self.is_enabled(action.as_ref()) => Lookup::Enabled(Arc::clone(action)),
            Some(action) => Lookup::Disabled(action.descriptor().clone()),
            None => Lookup::Unknown,
        }
    }

    fn catalogue(&self) -> Vec<CatalogueEntry> {
        self.actions
            .values()
            .map(|action| CatalogueEntry {
                descriptor: action.descriptor().clone(),
                enabled: self.is_enabled(action.as_ref()),
            })
            .collect()
    }
}

impl fmt::Debug for InMemoryRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryRegistry")
            .field("actions", &self.actions.keys().collect::<Vec<_>>())
            .field("enabled_dangerous", &self.enabled_dangerous)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("an Action named `{name}` is already registered")]
pub struct DuplicateAction {
    pub name: ActionName,
}
