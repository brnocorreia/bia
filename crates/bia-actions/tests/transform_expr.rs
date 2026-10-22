//! `transform.expr` through the `Action` trait.

mod common;

use bia_core::action::{Effect, ErrorClass};
use common::{action, execute};
use serde_json::json;

#[tokio::test]
async fn returns_a_number() {
    let output = execute(
        "transform.expr",
        json!({ "expression": "values.a + values.b", "values": { "a": 2, "b": 3 } }),
    )
    .await
    .unwrap();
    assert_eq!(output, json!(5));
}

#[tokio::test]
async fn returns_an_object() {
    let output = execute(
        "transform.expr",
        json!({
            "expression": "{'name': values.user.name.upperAscii(), 'admin': values.user.role == 'admin'}",
            "values": { "user": { "name": "ada", "role": "admin" } }
        }),
    )
    .await
    .unwrap();
    assert_eq!(output, json!({ "name": "ADA", "admin": true }));
}

#[tokio::test]
async fn returns_a_list() {
    let output = execute(
        "transform.expr",
        json!({
            "expression": "values.items.filter(i, i.qty > 1).map(i, i.sku)",
            "values": { "items": [{ "sku": "a", "qty": 1 }, { "sku": "b", "qty": 3 }] }
        }),
    )
    .await
    .unwrap();
    assert_eq!(output, json!(["b"]));
}

#[tokio::test]
async fn values_are_optional() {
    let output = execute("transform.expr", json!({ "expression": "[1, 2].size()" }))
        .await
        .unwrap();
    assert_eq!(output, json!(2));
}

#[tokio::test]
async fn an_expression_that_does_not_compile_is_fatal() {
    let error = execute("transform.expr", json!({ "expression": "values.a +" }))
        .await
        .unwrap_err();
    assert_eq!(error.class(), ErrorClass::Fatal);
    assert!(error.message().contains("does not compile"), "{error}");
}

#[tokio::test]
async fn an_expression_that_fails_to_evaluate_is_fatal() {
    for expression in [
        "values.missing",
        "values.a / 0",
        "values.a + 'x'",
        "input.a",
    ] {
        let error = execute(
            "transform.expr",
            json!({ "expression": expression, "values": { "a": 1 } }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.class(), ErrorClass::Fatal, "{expression}: {error}");
    }
}

#[tokio::test]
async fn invalid_params_are_fatal() {
    for params in [
        json!({}),
        json!({ "expression": 1 }),
        json!({ "expression": "1", "values": [1] }),
        json!({ "expression": "1", "extra": true }),
    ] {
        let error = execute("transform.expr", params.clone()).await.unwrap_err();
        assert_eq!(error.class(), ErrorClass::Fatal, "{params}: {error}");
        assert!(error.message().starts_with("invalid Params"), "{error}");
    }
}

#[test]
fn is_pure() {
    assert_eq!(action("transform.expr").descriptor().effect, Effect::Pure);
}

#[tokio::test]
async fn examples_produce_their_documented_output() {
    let action = action("transform.expr");
    for example in &action.descriptor().examples {
        let output = action
            .execute(
                common::call(serde_json::Value::Object(example.params.clone())),
                bia_core::action::CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(output, example.output, "{}", example.description);
    }
}
