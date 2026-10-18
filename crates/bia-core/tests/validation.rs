//! Layered validation of Definitions into Diagnostics.

// `allow-unwrap-in-tests` covers `#[test]` functions only, not the helpers they share.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use bia_core::action::InMemoryRegistry;
use bia_core::action::testing::FakeAction;
use bia_core::dsl::{QueueName, WorkflowId, yaml_to_json};
use bia_core::validation::{Code, Diagnostic, KnownResources, Limits, Severity, Target, Validator};
use schemars::json_schema;
use serde_json::{Value, json};

const DSL_DOC: &str = include_str!("../../../docs/dsl.md");

/// The Actions the Definitions below use, plus a dangerous one left disabled.
fn registry() -> InMemoryRegistry {
    let mut registry = InMemoryRegistry::new();
    let actions = [
        FakeAction::new("http.request").with_descriptor(|d| {
            d.param_schema = json_schema!({
                "type": "object",
                "required": ["method", "url"],
                "properties": {
                    "method": { "enum": ["GET", "POST", "PUT", "DELETE"] },
                    "url": { "type": "string" },
                    "headers": { "type": "object", "additionalProperties": { "type": "string" } },
                    "body": {},
                },
                "additionalProperties": false,
            });
        }),
        FakeAction::new("queue.publish").with_descriptor(|d| {
            d.param_schema = json_schema!({
                "type": "object",
                "required": ["queue", "body"],
                "properties": {
                    "queue": { "type": "string", "x-bia-resource": "queue" },
                    "body": {},
                },
                "additionalProperties": false,
            });
        }),
        FakeAction::new("flow.sleep").with_descriptor(|d| {
            d.param_schema = json_schema!({
                "type": "object",
                "required": ["duration"],
                "properties": { "duration": { "type": "string" } },
            });
        }),
        FakeAction::new("flow.subworkflow").with_descriptor(|d| {
            d.param_schema = json_schema!({
                "type": "object",
                "required": ["workflow"],
                "properties": {
                    "workflow": { "type": "string", "x-bia-resource": "workflow" },
                    "input": {},
                    "wait": { "type": "boolean" },
                },
            });
        }),
        FakeAction::new("human.approval").with_descriptor(|d| {
            d.param_schema = json_schema!({
                "type": "object",
                "required": ["title"],
                "properties": {
                    "title": { "type": "string" },
                    "timeout": { "type": "string" },
                },
            });
        }),
        FakeAction::new("exec.shell").with_descriptor(|d| d.dangerous = true),
    ];
    for action in actions {
        registry.register(Arc::new(action)).unwrap();
    }
    registry
}

fn resources() -> KnownResources {
    KnownResources::new()
        .queue(QueueName::new("emails").unwrap())
        .queue(QueueName::new("signups").unwrap())
        .workflow(WorkflowId::new("send-invoice").unwrap())
}

fn validate_with(target: Target, limits: Limits, document: Value) -> Vec<Diagnostic> {
    let registry = registry();
    let resources = resources();
    Validator::new(target, &registry, &resources)
        .with_limits(limits)
        .validate_value(document)
        .diagnostics
}

fn validate(target: Target, document: Value) -> Vec<Diagnostic> {
    validate_with(target, Limits::default(), document)
}

/// A Definition with these Steps.
fn steps(steps: Value) -> Value {
    json!({ "schema_version": 1, "id": "test", "steps": steps })
}

fn yaml(source: &str) -> Value {
    yaml_to_json(source).unwrap()
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code.as_str()).collect()
}

/// Returns the `index`-th fenced YAML block of `docs/dsl.md`.
fn doc_yaml_block(index: usize) -> &'static str {
    DSL_DOC
        .split("```yaml\n")
        .nth(index + 1)
        .and_then(|block| block.split("\n```").next())
        .expect("docs/dsl.md has that many YAML blocks")
}

#[test]
fn the_docs_example_is_valid_for_publishing() {
    let diagnostics = validate(Target::Publish, yaml(doc_yaml_block(0)));
    assert_eq!(diagnostics, []);
}

#[test]
fn every_mistake_is_reported_in_one_pass() {
    let diagnostics = validate(
        Target::Publish,
        yaml(
            r"
            schema_version: 1
            id: test
            steps:
              - id: fetch
                action: http.reqest
              - id: fetch
                action: flow.sleep
                params: { duration: 1s }
              - id: check
                action: flow.branch
                when: steps.fetch.output.ok &&
                then:
                  - id: notify
                    action: queue.publish
                    params: { queue: emals, body: '{{ steps.later.output }}' }
              - id: later
                action: flow.sleep
                params: { duration: 1s }
            ",
        ),
    );
    assert_eq!(
        codes(&diagnostics),
        [
            "unknown_action",
            "duplicate_step_id",
            "invalid_expression",
            "forward_step_reference",
            "unknown_queue",
        ]
    );
}

#[test]
fn duplicate_step_id_in_a_nested_block() {
    insta::assert_json_snapshot!(validate(
        Target::Draft,
        yaml(
            r"
            schema_version: 1
            id: test
            steps:
              - id: wait
                action: flow.sleep
                params: { duration: 1s }
              - id: guard
                action: flow.try
                body:
                  - id: loop
                    action: flow.foreach
                    items: '[1, 2]'
                    body:
                      - id: wait
                        action: flow.sleep
                        params: { duration: 1s }
                catch:
                  - id: guard
                    action: flow.sleep
                    params: { duration: 1s }
            ",
        ),
    ));
}

#[test]
fn reference_to_a_later_step() {
    insta::assert_json_snapshot!(validate(
        Target::Draft,
        steps(json!([
            {
                "id": "notify",
                "action": "queue.publish",
                "params": { "queue": "emails", "body": "{{ steps.fetch.output.body }}" },
            },
            { "id": "fetch", "action": "http.request", "params": { "method": "GET", "url": "https://x" } },
            { "id": "self_ref", "action": "flow.sleep", "if": "steps.self_ref.output.done", "params": { "duration": "1s" } },
        ])),
    ));
}

#[test]
fn reference_to_a_step_only_on_another_branch_arm() {
    insta::assert_json_snapshot!(validate(
        Target::Draft,
        yaml(
            r"
            schema_version: 1
            id: test
            steps:
              - id: check
                action: flow.branch
                when: 'true'
                then:
                  - id: vip
                    action: flow.sleep
                    params: { duration: 1s }
                else:
                  - id: basic
                    action: queue.publish
                    params: { queue: emails, body: '{{ steps.vip.output }}' }
              - id: after
                action: queue.publish
                params: { queue: emails, body: '{{ steps.basic.output }}' }
            ",
        ),
    ));
}

#[test]
fn expression_that_does_not_compile() {
    insta::assert_json_snapshot!(validate(
        Target::Draft,
        steps(json!([
            {
                "id": "check",
                "action": "flow.branch",
                "when": "input.count >",
                "then": [{ "id": "a", "action": "flow.sleep", "params": { "duration": "{{ input. }}" } }],
            },
            { "id": "long", "action": "flow.sleep", "if": "1 + ".repeat(3000) + "1", "params": { "duration": "1s" } },
        ])),
    ));
}

#[test]
fn secret_read_in_a_when() {
    insta::assert_json_snapshot!(validate(
        Target::Draft,
        steps(json!([
            {
                "id": "check",
                "action": "flow.branch",
                "when": "secrets.MODE == 'live' && size(secrets) > 0",
                "then": [{
                    "id": "call",
                    "action": "http.request",
                    "params": {
                        "method": "GET",
                        "url": "https://x",
                        "headers": { "Authorization": "Bearer {{ secrets.API_TOKEN }}" },
                    },
                }],
            },
        ])),
    ));
}

#[test]
fn params_that_violate_the_action_schema() {
    insta::assert_json_snapshot!(validate(
        Target::Draft,
        steps(json!([
            {
                "id": "call",
                "action": "http.request",
                "params": {
                    "method": "FETCH",
                    "headers": { "X-Count": 3 },
                    "retries": 2,
                },
            },
            // Templates resolve at execution time, so they are not held against the schema.
            {
                "id": "templated",
                "action": "http.request",
                "params": { "method": "{{ input.method }}", "url": "{{ input.url }}" },
            },
        ])),
    ));
}

#[test]
fn unknown_action_with_a_suggestion() {
    insta::assert_json_snapshot!(validate(
        Target::Draft,
        steps(json!([
            { "id": "call", "action": "http.reqeust", "params": { "url": "{{ input.url }}" } },
            { "id": "branch", "action": "flow.brnch" },
            { "id": "nothing_close", "action": "made.up" },
        ])),
    ));
}

#[test]
fn disabled_action_at_publish() {
    let definition = steps(json!([
        { "id": "run", "action": "exec.shell", "params": { "command": "ls" } },
    ]));
    let draft = validate(Target::Draft, definition.clone());
    assert_eq!(draft.len(), 1);
    assert_eq!(draft[0].code, Code::DisabledAction);
    assert_eq!(draft[0].severity, Severity::Warning);
    insta::assert_json_snapshot!(validate(Target::Publish, definition));
}

fn unknown_queue_definition() -> Value {
    yaml(
        r"
        schema_version: 1
        id: test
        triggers:
          - { id: incoming, type: queue, queue: signup }
        steps:
          - id: wait
            action: flow.sleep
            params: { duration: 1s }
          - id: check
            action: flow.branch
            when: 'true'
            then:
              - id: notify
                action: queue.publish
                params: { queue: emals, body: hi }
              - id: elsewhere
                action: queue.publish
                params: { queue: '{{ input.queue }}', body: hi }
              - id: child
                action: flow.subworkflow
                params: { workflow: send-invoce }
              - id: recurse
                action: flow.subworkflow
                params: { workflow: test }
        ",
    )
}

#[test]
fn unknown_queue_on_a_draft_is_a_warning() {
    let diagnostics = validate(Target::Draft, unknown_queue_definition());
    assert!(diagnostics.iter().all(|d| d.severity == Severity::Warning));
    insta::assert_json_snapshot!(diagnostics);
}

#[test]
fn unknown_queue_at_publish_is_an_error() {
    let diagnostics = validate(Target::Publish, unknown_queue_definition());
    assert!(diagnostics.iter().all(Diagnostic::is_error));
    assert_eq!(diagnostics[1].path, "/steps/1/then/0/params/queue");
    insta::assert_json_snapshot!(diagnostics);
}

#[test]
fn limits_exceeded() {
    let limits = Limits {
        max_steps: 4,
        max_nesting_depth: 1,
        max_iterations: 100,
        ..Limits::default()
    };
    insta::assert_json_snapshot!(validate_with(
        Target::Draft,
        limits,
        yaml(
            r"
            schema_version: 1
            id: test
            steps:
              - id: outer
                action: flow.foreach
                items: input.list
                max_iterations: 1000
                body:
                  - id: inner
                    action: flow.loop_until
                    until: 'true'
                    body:
                      - id: deepest
                        action: flow.sleep
                        params: { duration: 1s }
              - id: second
                action: flow.sleep
                params: { duration: 1s }
              - id: third
                action: flow.sleep
                params: { duration: 1s }
            ",
        ),
    ));
}

#[test]
fn scopes_follow_the_block_tree() {
    insta::assert_json_snapshot!(validate(
        Target::Draft,
        yaml(
            r"
            schema_version: 1
            id: test
            steps:
              - id: fan_out
                action: flow.parallel
                branches:
                  - - id: left
                      action: flow.sleep
                      params: { duration: 1s }
                  - - id: right
                      action: flow.sleep
                      params: { duration: '{{ steps.left.output }}' }
              - id: race
                action: flow.parallel
                join: any
                branches:
                  - - id: fast
                      action: flow.sleep
                      params: { duration: 1s }
              - id: each
                action: flow.foreach
                items: steps.left.output.list
                body:
                  - id: per_item
                    action: flow.sleep
                    params: { duration: '{{ item }}s {{ index }}' }
                  - id: uses_previous
                    action: flow.sleep
                    params: { duration: '{{ steps.per_item.output }}' }
              - id: poll
                action: flow.loop_until
                until: steps.attempt.output.done && steps.maybe.output.done && steps.poll.output
                body:
                  - id: attempt
                    action: flow.sleep
                    params: { duration: 1s }
                  - id: optional
                    action: flow.branch
                    when: 'true'
                    then:
                      - id: maybe
                        action: flow.sleep
                        params: { duration: 1s }
              - id: guarded
                action: flow.try
                body:
                  - id: risky
                    action: flow.sleep
                    params: { duration: 1s }
                catch:
                  - id: recover
                    action: flow.sleep
                    params: { duration: '{{ error.message }} {{ steps.risky.output }}' }
              - id: summary
                action: flow.sleep
                params:
                  duration: >-
                    {{ steps.right.output }} {{ steps.fast.output }} {{ steps.per_item.output }}
                    {{ steps.each.output }} {{ steps.attempt.output }} {{ steps.recover.output }}
                    {{ steps.guarded.output }} {{ item }} {{ error }} {{ steps.sumary.output }}
            on_error:
              - id: alert
                action: flow.sleep
                params: { duration: '{{ steps.summary.output }}' }
            ",
        ),
    ));
}

#[test]
fn shape_errors_are_diagnostics_with_paths() {
    let cases = [
        (
            "unknown_field",
            json!({ "schema_version": 1, "id": "t", "steps": [
                { "id": "a", "action": "flow.sleep", "retires": 3 },
            ]}),
        ),
        (
            "invalid_value",
            json!({ "schema_version": 1, "id": "t", "triggers": [
                { "id": "daily", "type": "cron", "expr": "* * * * *", "overlap": "skp" },
            ], "steps": []}),
        ),
        (
            "field_not_allowed",
            json!({ "schema_version": 1, "id": "t", "steps": [
                { "id": "a", "action": "flow.branch", "when": "true", "then": [], "params": {} },
            ]}),
        ),
        (
            "missing_field",
            json!({ "schema_version": 1, "id": "t", "steps": [{ "id": "a" }] }),
        ),
        ("missing_schema_version", json!({ "id": "t", "steps": [] })),
    ];
    let diagnostics: Vec<(&str, Vec<Diagnostic>)> = cases
        .into_iter()
        .map(|(name, document)| (name, validate(Target::Draft, document)))
        .collect();
    for (name, found) in &diagnostics {
        assert_eq!(codes(found), [*name]);
    }
    insta::assert_json_snapshot!(diagnostics);
}

#[test]
fn the_code_catalogue_is_stable() {
    let catalogue: Vec<&str> = Code::ALL.iter().map(|code| code.as_str()).collect();
    insta::assert_json_snapshot!(catalogue);
}

#[test]
fn every_code_is_documented() {
    for code in Code::ALL {
        assert!(
            DSL_DOC.contains(&format!("| `{code}` |")),
            "`{code}` is missing from the Diagnostic codes table in docs/dsl.md"
        );
    }
}

#[test]
fn diagnostics_serialise_as_documented() {
    let diagnostic = Diagnostic {
        path: "/steps/1/then/0/params/queue".into(),
        code: Code::UnknownQueue,
        severity: Severity::Error,
        message: "Queue 'emals' does not exist".into(),
        suggestion: Some("Did you mean 'emails'?".into()),
    };
    let documented: Value = DSL_DOC
        .split("## Diagnostics")
        .nth(1)
        .and_then(|section| section.split("```json\n").nth(1))
        .and_then(|block| block.split("\n```").next())
        .map(|json| serde_json::from_str(json).unwrap())
        .unwrap();
    assert_eq!(serde_json::to_value(&diagnostic).unwrap(), documented);
    assert_eq!(
        serde_json::from_value::<Diagnostic>(documented).unwrap(),
        diagnostic
    );
}

#[test]
fn action_names_in_the_registry_are_suggested_for_typos() {
    // The suggestion draws on the registry, so a new Action is suggested as soon as it exists.
    let mut registry = registry();
    registry
        .register(Arc::new(FakeAction::new("slack.post_message")))
        .unwrap();
    let resources = resources();
    let diagnostics = Validator::new(Target::Draft, &registry, &resources).validate_value(steps(
        json!([{ "id": "say", "action": "slack.post_mesage" }]),
    ));
    assert_eq!(
        diagnostics.diagnostics[0].suggestion.as_deref(),
        Some("Did you mean 'slack.post_message'?")
    );
}
