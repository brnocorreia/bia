//! Helpers shared by the Action tests.

#![allow(clippy::unwrap_used, dead_code, unreachable_pub)]

use std::sync::Arc;

use bia_core::action::{
    Action, ActionCall, ActionError, ActionRegistry, CancellationToken, IdempotencyKey, Lookup,
    Output, RunContext,
};
use bia_core::dsl::{Params, StepId, WorkflowId};
use serde_json::Value;

pub const IDEMPOTENCY_KEY: &str = "run_1:fetch:0";

pub fn action(name: &str) -> Arc<dyn Action> {
    let registry = bia_actions::builtin_registry().unwrap();
    match registry.lookup(name) {
        Lookup::Enabled(action) => action,
        other => panic!("expected `{name}` to be enabled, got {other:?}"),
    }
}

pub fn params(value: Value) -> Params {
    match value {
        Value::Object(map) => map,
        other => panic!("Params must be an object, got {other}"),
    }
}

pub fn call(params_value: Value) -> ActionCall {
    ActionCall {
        params: params(params_value),
        idempotency_key: IdempotencyKey::new(IDEMPOTENCY_KEY),
        context: RunContext {
            run_id: "run_1".into(),
            workflow_id: WorkflowId::new("uptime-check").unwrap(),
            version: 1,
            step_id: StepId::new("fetch").unwrap(),
            attempt: 1,
        },
    }
}

pub async fn execute(name: &str, params_value: Value) -> Result<Output, ActionError> {
    action(name)
        .execute(call(params_value), CancellationToken::new())
        .await
}
