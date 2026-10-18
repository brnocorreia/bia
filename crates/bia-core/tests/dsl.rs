//! Parsing Definitions and queue resources from JSON and YAML.

// `allow-unwrap-in-tests` covers `#[test]` functions only, not the helpers they share.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use bia_core::dsl::{
    Definition, Document, Duration, DurationUnit, ParseErrorKind, QueueResource, Step, StepKind,
    TriggerKind, yaml_to_json,
};
use serde_json::{Value, json};

const DSL_DOC: &str = include_str!("../../../docs/dsl.md");

/// Returns the `index`-th fenced YAML block of `docs/dsl.md`.
fn doc_yaml_block(index: usize) -> &'static str {
    DSL_DOC
        .split("```yaml\n")
        .nth(index + 1)
        .and_then(|block| block.split("\n```").next())
        .expect("docs/dsl.md has that many YAML blocks")
}

/// Every fixture that must parse, by name.
fn fixtures() -> Vec<(&'static str, Definition)> {
    vec![
        (
            "docs/dsl.md",
            Definition::from_yaml(doc_yaml_block(0)).unwrap(),
        ),
        (
            "blocks.yaml",
            Definition::from_yaml(include_str!("fixtures/blocks.yaml")).unwrap(),
        ),
        (
            "triggers.yaml",
            Definition::from_yaml(include_str!("fixtures/triggers.yaml")).unwrap(),
        ),
        (
            "minimal.json",
            Definition::from_json(include_str!("fixtures/minimal.json")).unwrap(),
        ),
    ]
}

fn walk<'a>(steps: &'a [Step], out: &mut Vec<&'a Step>) {
    for step in steps {
        out.push(step);
        match &step.kind {
            StepKind::Action { .. } => {}
            StepKind::Branch(b) => {
                walk(&b.then, out);
                walk(&b.r#else, out);
            }
            StepKind::Switch(s) => {
                for case in &s.cases {
                    walk(&case.steps, out);
                }
                walk(&s.default, out);
            }
            StepKind::Parallel(p) => {
                for branch in &p.branches {
                    walk(branch, out);
                }
            }
            StepKind::Foreach(f) => walk(&f.body, out),
            StepKind::LoopUntil(l) => walk(&l.body, out),
            StepKind::Try(t) => {
                walk(&t.body, out);
                walk(&t.catch, out);
            }
        }
    }
}

fn all_steps(definition: &Definition) -> Vec<&Step> {
    let mut steps = Vec::new();
    walk(&definition.steps, &mut steps);
    walk(&definition.on_error, &mut steps);
    steps
}

#[test]
fn docs_example_parses_the_same_from_yaml_and_json() {
    let from_yaml = Definition::from_yaml(doc_yaml_block(0)).unwrap();
    let from_json = Definition::from_json(include_str!("fixtures/docs_example.json")).unwrap();
    assert_eq!(from_yaml, from_json);

    assert_eq!(from_yaml.id.as_str(), "onboarding-email");
    assert_eq!(from_yaml.triggers.len(), 4);
    let actions: Vec<_> = from_yaml.steps.iter().map(Step::action).collect();
    assert_eq!(
        actions,
        [
            "http.request",
            "flow.branch",
            "flow.sleep",
            "human.approval"
        ]
    );
}

#[test]
fn yaml_converts_to_the_json_fixture() {
    let converted = yaml_to_json(doc_yaml_block(0)).unwrap();
    let fixture: Value = serde_json::from_str(include_str!("fixtures/docs_example.json")).unwrap();
    assert_eq!(converted, fixture);
}

#[test]
fn fixtures_round_trip_through_canonical_json() {
    for (name, definition) in fixtures() {
        let json = serde_json::to_string(&definition).unwrap();
        let reparsed = Definition::from_json(&json)
            .unwrap_or_else(|err| panic!("{name} does not re-parse: {err}\n{json}"));
        assert_eq!(reparsed, definition, "{name} changed in a round trip");

        // Serialising is stable: the canonical form of the canonical form is itself.
        assert_eq!(serde_json::to_string(&reparsed).unwrap(), json, "{name}");
    }
}

#[test]
fn fixtures_cover_every_block_and_trigger_type() {
    let mut blocks = BTreeSet::new();
    let mut triggers = BTreeSet::new();
    for (_, definition) in fixtures() {
        for step in all_steps(&definition) {
            if step.kind.is_block() {
                blocks.insert(step.action().to_owned());
            }
        }
        for trigger in &definition.triggers {
            triggers.insert(trigger.kind.type_name());
        }
    }
    let expected_blocks = [
        "flow.branch",
        "flow.foreach",
        "flow.loop_until",
        "flow.parallel",
        "flow.switch",
        "flow.try",
    ];
    assert_eq!(blocks, expected_blocks.map(String::from).into());
    assert_eq!(
        triggers,
        ["cron", "event", "manual", "queue", "webhook"].into()
    );
}

#[test]
fn canonical_json_omits_absent_fields_and_flattens_steps() {
    let definition = Definition::from_yaml(
        r#"
schema_version: 1
id: shape
triggers:
  - { id: by_hand, type: manual }
steps:
  - id: check
    action: flow.branch
    when: input.ok
    then:
      - { id: hello, action: log.info, params: { message: hi } }
"#,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&definition).unwrap(),
        json!({
            "schema_version": 1,
            "id": "shape",
            "triggers": [{ "id": "by_hand", "type": "manual" }],
            "steps": [{
                "id": "check",
                "action": "flow.branch",
                "when": "input.ok",
                "then": [{ "id": "hello", "action": "log.info", "params": { "message": "hi" } }],
            }],
        })
    );
}

#[test]
fn blocks_and_triggers_are_typed() {
    let definition = Definition::from_yaml(include_str!("fixtures/triggers.yaml")).unwrap();
    let TriggerKind::Cron(cron) = &definition.triggers[0].kind else {
        panic!("first trigger is cron");
    };
    assert_eq!(cron.jitter, Some("30s".parse().unwrap()));

    let definition = Definition::from_yaml(include_str!("fixtures/blocks.yaml")).unwrap();
    let StepKind::Switch(switch) = &definition.steps[1].kind else {
        panic!("second step is flow.switch");
    };
    assert_eq!(
        switch.cases[1].equals,
        json!({ "nested": [1, 2.5, true, null] })
    );
    assert_eq!(definition.steps[0].timeout, Some("60m".parse().unwrap()));
}

#[test]
fn missing_schema_version_is_rejected_at_its_path() {
    let err = Definition::from_json(r#"{ "id": "x", "steps": [] }"#).unwrap_err();
    assert_eq!(err.path, "/schema_version");
    assert_eq!(err.kind, ParseErrorKind::MissingSchemaVersion);

    let err = Definition::from_yaml("id: x\nsteps: []\n").unwrap_err();
    assert_eq!(err.path, "/schema_version");
    assert_eq!(err.kind, ParseErrorKind::MissingSchemaVersion);
}

#[test]
fn unsupported_schema_version_is_rejected_at_its_path() {
    for version in [json!(0), json!(2), json!("1"), json!(1.5)] {
        let err =
            Definition::from_value(json!({ "schema_version": version, "id": "x", "steps": [] }))
                .unwrap_err();
        assert_eq!(err.path, "/schema_version", "{version}");
        assert_eq!(
            err.kind,
            ParseErrorKind::UnsupportedSchemaVersion {
                found: version.to_string()
            }
        );
    }
}

/// Parses a one-Step Definition whose Step is `step`.
fn parse_step(step: Value) -> Result<Definition, bia_core::dsl::ParseError> {
    Definition::from_value(json!({ "schema_version": 1, "id": "x", "steps": [step] }))
}

#[test]
fn shape_errors_carry_the_json_path_of_nested_values() {
    let err = parse_step(json!({
        "id": "outer",
        "action": "flow.branch",
        "when": "true",
        "then": [{ "id": "inner", "action": "http.request", "retry": { "max_attempts": 2, "backoff": "linear" } }],
    }))
    .unwrap_err();
    assert_eq!(err.path, "/steps/0/then/0/retry/backoff");
    assert!(matches!(err.kind, ParseErrorKind::Shape { .. }), "{err}");

    let err = parse_step(json!({ "id": "x", "action": "flow.sleep", "timeout": "5 minutes" }))
        .unwrap_err();
    assert_eq!(err.path, "/steps/0/timeout");

    let err = parse_step(json!({ "id": "Bad-Id", "action": "flow.sleep" })).unwrap_err();
    assert_eq!(err.path, "/steps/0/id");
}

#[test]
fn fields_that_do_not_fit_the_action_are_rejected() {
    let cases = [
        (
            json!({ "id": "x", "action": "http.request", "then": [] }),
            "field `then` is not allowed on a `http.request` Step",
        ),
        (
            json!({ "id": "x", "action": "flow.branch", "when": "true", "then": [], "params": {} }),
            "field `params` is not allowed on a `flow.branch` Step",
        ),
        (
            json!({ "id": "x", "action": "flow.try", "body": [] }),
            "missing field `catch` for a `flow.try` Step",
        ),
    ];
    for (step, message) in cases {
        let err = parse_step(step.clone()).unwrap_err();
        assert_eq!(err.path, "/steps/0", "{step}");
        assert!(err.to_string().contains(message), "{err} for {step}");
    }

    let err =
        parse_step(json!({ "id": "x", "action": "http.request", "retires": {} })).unwrap_err();
    assert_eq!(err.path, "/steps/0/retires");
    assert!(err.to_string().contains("unknown field `retires`"), "{err}");
}

#[test]
fn trigger_fields_are_checked_against_the_type() {
    let parse = |trigger: Value| {
        Definition::from_value(
            json!({ "schema_version": 1, "id": "x", "triggers": [trigger], "steps": [] }),
        )
        .unwrap_err()
    };

    let err = parse(json!({ "id": "t", "type": "cron" }));
    assert_eq!(err.path, "/triggers/0");
    assert!(
        err.to_string()
            .contains("missing field `expr` for a `cron` Trigger"),
        "{err}"
    );

    let err = parse(json!({ "id": "t", "type": "manual", "queue": "q" }));
    assert!(
        err.to_string()
            .contains("`queue` is not allowed on a `manual` Trigger"),
        "{err}"
    );

    let err = parse(json!({ "id": "t", "type": "poll" }));
    assert_eq!(err.path, "/triggers/0/type");
}

#[test]
fn unknown_top_level_fields_are_rejected() {
    let err =
        Definition::from_value(json!({ "schema_version": 1, "id": "x", "steps": [], "stpes": [] }))
            .unwrap_err();
    assert_eq!(err.path, "/stpes");
    assert!(err.to_string().contains("unknown field `stpes`"), "{err}");
}

#[test]
fn syntax_errors_are_reported() {
    let err = Definition::from_json(r#"{ "schema_version": 1, "#).unwrap_err();
    assert!(matches!(err.kind, ParseErrorKind::Syntax { .. }), "{err}");

    let err = Definition::from_yaml("schema_version: 1\nsteps: [\n").unwrap_err();
    assert!(matches!(err.kind, ParseErrorKind::Syntax { .. }), "{err}");

    let err = Definition::from_yaml("schema_version: 1\nid: a\nid: b\nsteps: []\n").unwrap_err();
    assert!(
        matches!(err.kind, ParseErrorKind::Syntax { .. }),
        "duplicate keys: {err}"
    );
}

#[test]
fn yaml_follows_yaml_1_2_scalars() {
    // YAML 1.1 would read `on`, `yes` and `no` as booleans.
    let value = yaml_to_json("a: on\nb: yes\nc: no\nd: true\ne: 0 9 * * *\n").unwrap();
    assert_eq!(
        value,
        json!({ "a": "on", "b": "yes", "c": "no", "d": true, "e": "0 9 * * *" })
    );
}

#[test]
fn queue_resources_parse_from_the_docs_example() {
    let queue = QueueResource::from_yaml(doc_yaml_block(1)).unwrap();
    assert_eq!(queue.name.as_str(), "emails");
    assert_eq!(queue.max_retries, Some(5));
    assert_eq!(queue.visibility_timeout, Some("30s".parse().unwrap()));
    assert_eq!(queue.dead_letter.unwrap().as_str(), "emails-dlq");

    let round_trip: QueueResource =
        QueueResource::from_json(&serde_json::to_string(&queue_minimal()).unwrap()).unwrap();
    assert_eq!(round_trip, queue_minimal());
}

fn queue_minimal() -> QueueResource {
    QueueResource::from_yaml(include_str!("fixtures/queue.yaml")).unwrap()
}

#[test]
fn documents_are_told_apart_by_kind() {
    assert!(matches!(
        Document::from_yaml(doc_yaml_block(0)).unwrap(),
        Document::Workflow(_)
    ));
    assert!(matches!(
        Document::from_yaml(doc_yaml_block(1)).unwrap(),
        Document::Queue(_)
    ));

    let err = Document::from_value(json!({ "kind": "workflow", "name": "x" })).unwrap_err();
    assert_eq!(err.path, "/kind");
    assert!(matches!(err.kind, ParseErrorKind::UnknownKind { .. }));

    // A Definition never has `kind`, so a stray one is not silently ignored.
    let err =
        Document::from_value(json!({ "schema_version": 1, "id": "x", "steps": [], "kind": 1 }))
            .unwrap_err();
    assert_eq!(err.path, "/kind");
}

#[test]
fn durations_accept_every_unit() {
    let cases = [
        ("500ms", 500, DurationUnit::Milliseconds),
        ("30s", 30_000, DurationUnit::Seconds),
        ("5m", 300_000, DurationUnit::Minutes),
        ("2h", 7_200_000, DurationUnit::Hours),
        ("3d", 259_200_000, DurationUnit::Days),
        ("0s", 0, DurationUnit::Seconds),
    ];
    for (text, millis, unit) in cases {
        let duration: Duration = text.parse().unwrap();
        assert_eq!(duration.as_millis(), millis, "{text}");
        assert_eq!(duration.unit(), unit, "{text}");
        assert_eq!(duration.to_string(), text, "{text} keeps its spelling");
    }
    assert_eq!("1m".parse::<Duration>(), "60s".parse::<Duration>());
}

#[test]
fn durations_reject_malformed_strings() {
    let malformed = [
        "",
        "s",
        "30",
        "30 s",
        " 30s",
        "30s ",
        "-5s",
        "+5s",
        "1.5h",
        "05s",
        "5S",
        "5sec",
        "5w",
        "5ms5",
        "1h30m",
        "99999999999999999999d",
        "213503982334602d",
    ];
    for text in malformed {
        assert!(
            text.parse::<Duration>().is_err(),
            "{text:?} should be rejected"
        );
    }

    let err = parse_step(json!({ "id": "x", "action": "flow.sleep", "timeout": 30 })).unwrap_err();
    assert_eq!(err.path, "/steps/0/timeout", "a number is not a Duration");
}
