//! The generated JSON Schema: its snapshot, and agreement with the parser and `docs/dsl.md`.

#![allow(clippy::unwrap_used)]

use bia_core::dsl::{
    Definition, QueueResource, definition_schema, queue_resource_schema, yaml_to_json,
};
use jsonschema::Validator;
use serde_json::{Value, json};

const DSL_DOC: &str = include_str!("../../../docs/dsl.md");

fn doc_yaml_block(index: usize) -> &'static str {
    DSL_DOC
        .split("```yaml\n")
        .nth(index + 1)
        .and_then(|block| block.split("\n```").next())
        .expect("docs/dsl.md has that many YAML blocks")
}

fn compile(schema: &Value) -> Validator {
    jsonschema::draft202012::new(schema).expect("the generated schema is a valid JSON Schema")
}

fn valid_definitions() -> Vec<(&'static str, Value)> {
    vec![
        ("docs/dsl.md", yaml_to_json(doc_yaml_block(0)).unwrap()),
        (
            "blocks.yaml",
            yaml_to_json(include_str!("fixtures/blocks.yaml")).unwrap(),
        ),
        (
            "triggers.yaml",
            yaml_to_json(include_str!("fixtures/triggers.yaml")).unwrap(),
        ),
        (
            "minimal.json",
            serde_json::from_str(include_str!("fixtures/minimal.json")).unwrap(),
        ),
    ]
}

#[test]
fn definition_schema_snapshot() {
    insta::assert_json_snapshot!("definition_schema", definition_schema());
}

#[test]
fn queue_resource_schema_snapshot() {
    insta::assert_json_snapshot!("queue_resource_schema", queue_resource_schema());
}

#[test]
fn docs_and_fixtures_satisfy_the_schema() {
    let validator = compile(&definition_schema());
    for (name, value) in valid_definitions() {
        let errors: Vec<_> = validator
            .iter_errors(&value)
            .map(|e| format!("{}: {e}", e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{name} fails the schema: {errors:#?}");

        let canonical = serde_json::to_value(Definition::from_value(value).unwrap()).unwrap();
        assert!(validator.is_valid(&canonical), "canonical {name}");
    }

    let queues = compile(&queue_resource_schema());
    for yaml in [doc_yaml_block(1), include_str!("fixtures/queue.yaml")] {
        let value = yaml_to_json(yaml).unwrap();
        assert!(queues.is_valid(&value), "{yaml}");
        assert!(QueueResource::from_value(value).is_ok());
    }
}

#[test]
fn schema_and_parser_reject_the_same_shapes() {
    let definition = |steps: Value| json!({ "schema_version": 1, "id": "x", "steps": steps });
    let step = |extra: Value| {
        let mut step = json!({ "id": "s", "action": "http.request" });
        step.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        definition(json!([step]))
    };
    let invalid = [
        ("no schema_version", json!({ "id": "x", "steps": [] })),
        (
            "unsupported schema_version",
            json!({ "schema_version": 2, "id": "x", "steps": [] }),
        ),
        ("no steps", json!({ "schema_version": 1, "id": "x" })),
        (
            "unknown top-level field",
            json!({ "schema_version": 1, "id": "x", "steps": [], "kind": "workflow" }),
        ),
        (
            "bad workflow id",
            json!({ "schema_version": 1, "id": "X", "steps": [] }),
        ),
        ("bad step id", step(json!({ "id": "1st" }))),
        ("bad action name", step(json!({ "action": "request" }))),
        ("unknown step field", step(json!({ "retries": 3 }))),
        ("block field on an Action", step(json!({ "then": [] }))),
        ("params not an object", step(json!({ "params": [] }))),
        ("malformed timeout", step(json!({ "timeout": "30" }))),
        (
            "bad backoff",
            step(json!({ "retry": { "max_attempts": 2, "backoff": "linear" } })),
        ),
        ("retry without max_attempts", step(json!({ "retry": {} }))),
        (
            "params on a Block",
            definition(
                json!([{ "id": "b", "action": "flow.branch", "when": "x", "then": [], "params": {} }]),
            ),
        ),
        (
            "Block without a required field",
            definition(json!([{ "id": "b", "action": "flow.foreach", "body": [] }])),
        ),
        (
            "bad join",
            definition(
                json!([{ "id": "p", "action": "flow.parallel", "branches": [], "join": "most" }]),
            ),
        ),
        (
            "unknown switch case field",
            definition(
                json!([{ "id": "w", "action": "flow.switch", "value": "x", "cases": [{ "equals": 1, "steps": [], "then": [] }] }]),
            ),
        ),
        (
            "unknown trigger type",
            json!({ "schema_version": 1, "id": "x", "steps": [], "triggers": [{ "id": "t", "type": "poll" }] }),
        ),
        (
            "field from another trigger type",
            json!({ "schema_version": 1, "id": "x", "steps": [], "triggers": [{ "id": "t", "type": "manual", "expr": "* * * * *" }] }),
        ),
        (
            "cron without expr",
            json!({ "schema_version": 1, "id": "x", "steps": [], "triggers": [{ "id": "t", "type": "cron" }] }),
        ),
    ];

    let validator = compile(&definition_schema());
    for (name, value) in invalid {
        assert!(!validator.is_valid(&value), "schema accepts {name}");
        assert!(
            Definition::from_value(value).is_err(),
            "parser accepts {name}"
        );
    }
}
