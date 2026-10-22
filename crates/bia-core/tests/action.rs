//! The `Action` trait and the Action Registry, driven through the fake Action.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use bia_core::action::testing::FakeAction;
use bia_core::action::{
    Action, ActionCall, ActionError, ActionExample, ActionRegistry, CancellationToken, Effect,
    ErrorClass, IdempotencyKey, InMemoryRegistry, Lookup, RunContext,
};
use bia_core::dsl::{ActionName, Params, StepId, WorkflowId};
use schemars::json_schema;
use serde_json::{Value, json};

fn params(value: Value) -> Params {
    match value {
        Value::Object(map) => map,
        other => panic!("Params must be an object, got {other}"),
    }
}

fn call(params_value: Value) -> ActionCall {
    ActionCall {
        params: params(params_value),
        idempotency_key: IdempotencyKey::new("run_1:fetch:0"),
        context: RunContext {
            run_id: "run_1".into(),
            workflow_id: WorkflowId::new("onboarding-email").unwrap(),
            version: 3,
            step_id: StepId::new("fetch").unwrap(),
            attempt: 1,
        },
    }
}

fn enabled(registry: &impl ActionRegistry, name: &str) -> Arc<dyn Action> {
    match registry.lookup(name) {
        Lookup::Enabled(action) => action,
        other => panic!("expected `{name}` to be enabled, got {other:?}"),
    }
}

#[tokio::test]
async fn a_registered_action_is_looked_up_and_executed_by_name() {
    let fake = Arc::new(FakeAction::new("test.echo"));
    let mut registry = InMemoryRegistry::new();
    registry.register(fake.clone()).unwrap();

    let action = enabled(&registry, "test.echo");
    let output = action
        .execute(call(json!({ "greeting": "hi" })), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(output, json!({ "greeting": "hi" }));
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].idempotency_key.as_str(), "run_1:fetch:0");
    assert_eq!(calls[0].context.step_id.as_str(), "fetch");
}

#[test]
fn unknown_names_are_reported() {
    let registry = InMemoryRegistry::new();
    assert!(matches!(registry.lookup("test.missing"), Lookup::Unknown));
}

#[test]
fn registering_a_name_twice_is_refused() {
    let mut registry = InMemoryRegistry::new();
    registry
        .register(Arc::new(FakeAction::new("test.echo")))
        .unwrap();

    let error = registry
        .register(Arc::new(FakeAction::new("test.echo")))
        .unwrap_err();
    assert_eq!(error.name.as_str(), "test.echo");
}

#[tokio::test]
async fn errors_keep_their_classification() {
    for (error, class) in [
        (
            ActionError::retryable("connection reset"),
            ErrorClass::Retryable,
        ),
        (ActionError::fatal("missing `url`"), ErrorClass::Fatal),
    ] {
        let fake = FakeAction::new("test.fail").failing(error.clone());
        let returned = fake
            .execute(call(json!({})), CancellationToken::new())
            .await
            .unwrap_err();

        assert_eq!(returned.class(), class);
        assert_eq!(returned.is_retryable(), class == ErrorClass::Retryable);
        assert_eq!(returned, error);
    }
}

#[test]
fn errors_serialise_with_their_classification() {
    let error = ActionError::fatal("not found").with_details(json!({ "status": 404 }));

    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(
        wire,
        json!({ "class": "fatal", "message": "not found", "details": { "status": 404 } })
    );
    assert_eq!(serde_json::from_value::<ActionError>(wire).unwrap(), error);
    assert_eq!(error.to_string(), "fatal error: not found");
}

#[tokio::test]
async fn a_running_action_observes_cancellation() {
    let fake = FakeAction::new("test.wait").until_cancelled();
    let token = CancellationToken::new();

    let (result, ()) = tokio::join!(fake.execute(call(json!({})), token.clone()), async {
        token.cancel()
    });

    assert!(fake.saw_cancellation());
    assert_eq!(result.unwrap_err().class(), ErrorClass::Retryable);
}

#[test]
fn dangerous_actions_are_disabled_until_enabled() {
    let exec = || {
        Arc::new(FakeAction::new("exec.process").with_descriptor(|d| {
            d.dangerous = true;
            d.effect = Effect::SideEffecting;
        }))
    };

    let mut registry = InMemoryRegistry::new();
    registry.register(exec()).unwrap();
    match registry.lookup("exec.process") {
        Lookup::Disabled(descriptor) => assert!(descriptor.dangerous),
        other => panic!("expected `exec.process` to be disabled, got {other:?}"),
    }
    assert!(!registry.catalogue()[0].enabled);

    registry.enable(ActionName::new("exec.process").unwrap());
    enabled(&registry, "exec.process");
    assert!(registry.catalogue()[0].enabled);
}

#[test]
fn the_catalogue_lists_every_descriptor_as_json() {
    let mut registry = InMemoryRegistry::new();
    registry
        .register(Arc::new(FakeAction::new("test.echo").with_descriptor(
            |d| {
                d.description = "Returns its Params.".into();
                d.param_schema = json_schema!({
                    "type": "object",
                    "properties": { "greeting": { "type": "string" } },
                    "required": ["greeting"],
                });
                d.output_schema = json_schema!({ "type": "object" });
                d.examples.push(ActionExample::new(
                    "Echo a greeting",
                    params(json!({ "greeting": "hi" })),
                    json!({ "greeting": "hi" }),
                ));
            },
        )))
        .unwrap();
    registry
        .register(Arc::new(FakeAction::new("exec.shell").with_descriptor(
            |d| {
                d.description = "Runs a shell command.".into();
                d.effect = Effect::SideEffecting;
                d.dangerous = true;
            },
        )))
        .unwrap();

    insta::assert_json_snapshot!("catalogue", registry.catalogue());
}

#[test]
fn call_debug_output_hides_param_values() {
    let debug = format!("{:?}", call(json!({ "token": "s3cr3t" })));
    assert!(debug.contains("token"));
    assert!(!debug.contains("s3cr3t"));
}
