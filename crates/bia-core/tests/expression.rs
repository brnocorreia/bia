//! Expressions and Templates through the `bia-core` public API.

use std::collections::BTreeSet;

use bia_core::expression::{
    CompileErrorKind, EvalError, Expression, Limits, References, Scope, Template,
};
use serde_json::{Value, json};

fn scope() -> Scope {
    Scope::new()
        .with_input(&json!({
            "user": { "name": "Ada", "plan": "vip" },
            "email": "ada@example.com",
            "tags": ["a", "b", "c"],
            "ratio": 0.5,
        }))
        .with_step_output("a", &json!({ "count": 3, "items": [1, 2, 3] }))
        .with_trigger(&json!({ "type": "manual" }))
        .with_run(&json!({ "id": "run_123", "version": 2, "attempt": 1 }))
}

fn eval(source: &str) -> Result<Value, EvalError> {
    Expression::compile(source)
        .expect("Expression compiles")
        .evaluate(&scope())
}

fn render(source: &str) -> Value {
    Template::parse(source)
        .expect("Template parses")
        .evaluate(&scope())
        .expect("Template evaluates")
}

#[test]
fn single_expression_templates_keep_their_type() {
    assert_eq!(render("{{ steps.a.output.count }}"), json!(3));
    assert_eq!(
        render("{{ input.user }}"),
        json!({ "name": "Ada", "plan": "vip" })
    );
    assert_eq!(render("{{steps.a.output.items}}"), json!([1, 2, 3]));
    assert_eq!(render("{{ input.ratio }}"), json!(0.5));
    assert_eq!(render("{{ null }}"), Value::Null);
    assert!(
        Template::parse("{{ input.user }}")
            .unwrap()
            .is_single_expression()
    );
}

#[test]
fn mixed_templates_render_strings() {
    assert_eq!(render("id: {{ run.id }}"), json!("id: run_123"));
    assert_eq!(
        render("{{ input.user.name }} has {{ steps.a.output.count }} items"),
        json!("Ada has 3 items")
    );
    assert_eq!(
        render("tags={{ input.tags }}"),
        json!(r#"tags=["a","b","c"]"#)
    );
    assert_eq!(render(" {{ steps.a.output.count }}"), json!(" 3"));
    assert_eq!(render("no segments"), json!("no segments"));
    assert_eq!(render(""), json!(""));
    assert!(!Template::parse(" {{ 1 }}").unwrap().is_single_expression());
}

#[test]
fn template_segments_may_contain_braces_and_string_delimiters() {
    assert_eq!(render("{{ {'a': {'b': 1}} }}"), json!({ "a": { "b": 1 } }));
    assert_eq!(render("{{ '}}' }}"), json!("}}"));
    assert_eq!(render("{{ \"a\\\"}}\" }}"), json!("a\"}}"));
    assert_eq!(render("{{ r'\\' }}"), json!("\\"));
    assert_eq!(render("{{ '''x}}''' }}"), json!("x}}"));
    assert_eq!(
        render("literal {{ '{{' }} braces }}"),
        json!("literal {{ braces }}")
    );
}

#[test]
fn numbers_map_between_json_and_cel() {
    assert_eq!(eval("steps.a.output.count + 1"), Ok(json!(4)));
    assert_eq!(eval("input.ratio * 2.0"), Ok(json!(1.0)));
    assert_eq!(eval("size(input.tags)"), Ok(json!(3)));
    assert_eq!(
        eval("18446744073709551615u"),
        Ok(json!(18446744073709551615u64))
    );
    assert_eq!(eval("b'hi'"), Ok(json!("aGk=")));
    assert_eq!(
        eval("timestamp('2026-01-02T03:04:05Z')"),
        Ok(json!("2026-01-02T03:04:05+00:00"))
    );
    assert_eq!(eval("duration('1m30s')"), Ok(json!("90s")));
    assert_eq!(eval("duration('-1.5s')"), Ok(json!("-1.5s")));
    assert_eq!(eval("{1: 'one'}"), Ok(json!({ "1": "one" })));
    assert!(matches!(eval("1.0 / 0.0"), Err(EvalError::NotJson { .. })));
}

#[test]
fn enabled_extensions() {
    assert_eq!(eval("'TacoCat'.lowerAscii()"), Ok(json!("tacocat")));
    assert_eq!(eval("input.tags.join('-')"), Ok(json!("a-b-c")));
    assert_eq!(eval("math.abs(-5)"), Ok(json!(5)));
    assert_eq!(eval("math.floor(2.7)"), Ok(json!(2.0)));
    assert_eq!(eval("base64.encode(b'hi')"), Ok(json!("aGk=")));
    assert_eq!(eval("input.email.matches('^[a-z]+@')"), Ok(json!(true)));
}

#[test]
fn conditions_evaluate_to_booleans() {
    let condition = |source: &str| {
        Expression::compile(source)
            .unwrap()
            .evaluate_condition(&scope())
    };
    assert_eq!(condition("input.user.plan == 'vip'"), Ok(true));
    assert_eq!(condition("steps.a.output.count > 5"), Ok(false));
    assert_eq!(condition("input.tags.exists(t, t == 'b')"), Ok(true));
    assert_eq!(
        condition("steps.a.output.count"),
        Err(EvalError::NotABoolean { got: "int".into() })
    );
    assert_eq!(
        condition("input.user"),
        Err(EvalError::NotABoolean { got: "map".into() })
    );
}

#[test]
fn invalid_cel_is_a_compile_error_with_a_position() {
    let error = Expression::compile("input.user ==").unwrap_err();
    assert_eq!(error.kind, CompileErrorKind::Syntax);
    assert_eq!((error.line, error.column), (1, 14));
    assert_eq!(error.offset, 13);

    let error = Expression::compile("input.a &&\n  )").unwrap_err();
    assert_eq!(error.kind, CompileErrorKind::Syntax);
    assert_eq!((error.line, error.column), (2, 3));

    assert_eq!(
        Expression::compile("  ").unwrap_err().kind,
        CompileErrorKind::Empty
    );
    for source in [
        "(((",
        "[1, 2",
        "'unterminated",
        "a.b.",
        "1 +* 2",
        "{{",
        "}}",
    ] {
        let error = Expression::compile(source).unwrap_err();
        assert_eq!(error.kind, CompileErrorKind::Syntax, "{source}");
    }
}

#[test]
fn template_errors_point_into_the_template() {
    let error = Template::parse("id: {{ run.id == ) }}").unwrap_err();
    assert_eq!(error.kind, CompileErrorKind::Syntax);
    assert_eq!(error.offset, 17);
    assert_eq!((error.line, error.column), (1, 18));

    let error = Template::parse("é {{ é }}").unwrap_err();
    assert_eq!(error.kind, CompileErrorKind::Syntax);
    assert_eq!(error.offset, 6);
    assert_eq!((error.line, error.column), (1, 6));

    let error = Template::parse("a {{ run.id ").unwrap_err();
    assert_eq!(error.kind, CompileErrorKind::UnclosedTemplate);
    assert_eq!(error.offset, 2);

    let error = Template::parse("a {{ '}} ").unwrap_err();
    assert_eq!(error.kind, CompileErrorKind::UnclosedTemplate);

    let error = Template::parse("line one\n{{   }}").unwrap_err();
    assert_eq!(error.kind, CompileErrorKind::Empty);
    assert_eq!((error.line, error.column), (2, 1));
}

#[test]
fn limits_reject_oversized_and_deeply_nested_sources() {
    let limits = Limits {
        max_source_len: 16,
        ..Limits::default()
    };
    let error = Expression::compile_with("input.user.name == 'Ada'", &limits).unwrap_err();
    assert_eq!(error.kind, CompileErrorKind::TooLong);

    let nested = format!("{}1{}", "[".repeat(500), "]".repeat(500));
    let error = Expression::compile(&nested).unwrap_err();
    assert_eq!(error.kind, CompileErrorKind::TooDeep);

    let chain = vec!["1"; 200].join(" + ");
    let error = Expression::compile(&chain).unwrap_err();
    assert_eq!(error.kind, CompileErrorKind::TooDeep);
}

/// Test threads have the 2 MiB stack Tokio workers have. Whatever compiles
/// must evaluate on one, in a debug build, without overflowing it.
#[test]
fn accepted_nesting_evaluates_on_a_small_stack() {
    let shapes: &[(&str, &str, &str)] = &[
        ("(", "1", ")"),
        ("[", "1", "]"),
        ("{1: ", "1", "}"),
        ("size(", "'a'", ")"),
        ("-", "1", ""),
        ("!", "true", ""),
        ("true ? 1 : ", "1", ""),
        ("[1].map(x, ", "1", ")"),
        ("[1].all(x, ", "true", ")"),
        ("[1].exists_one(x, ", "true", ")"),
        ("[1].filter(x, ", "true", ")"),
    ];
    for (open, atom, close) in shapes {
        for n in [1, 5, 10, 20, 31, 32, 33, 64, 1000] {
            let source = format!("{}{atom}{}", open.repeat(n), close.repeat(n));
            if let Ok(expression) = Expression::compile(&source) {
                let _ = expression.evaluate(&scope());
            }
        }
    }
    for operator in [" + ", " && ", " || ", " == ", "."] {
        for n in [2, 10, 31, 32, 33, 1000] {
            let source = vec!["a"; n].join(operator);
            if let Ok(expression) = Expression::compile(&source) {
                let _ = expression.evaluate(&scope());
            }
        }
    }
}

fn refs(steps: &[&str], secrets: &[&str]) -> References {
    References {
        steps: steps.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        secrets: secrets
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        ..References::default()
    }
}

#[test]
fn extraction_lists_exactly_the_referenced_steps_and_secrets() {
    let cases: &[(&str, References)] = &[
        ("input.email", refs(&[], &[])),
        ("steps.a.output.count", refs(&["a"], &[])),
        (
            "steps.a.output.x + steps.b.output.y",
            refs(&["a", "b"], &[]),
        ),
        ("steps.a.output.x + steps.a.output.y", refs(&["a"], &[])),
        ("steps['a'].output", refs(&["a"], &[])),
        ("steps.?a.orValue(1)", refs(&["a"], &[])),
        ("has(steps.a.output.body)", refs(&["a"], &[])),
        ("secrets.API_TOKEN", refs(&[], &["API_TOKEN"])),
        (
            "steps.fetch.output.items.map(i, i.id + secrets.SALT)",
            refs(&["fetch"], &["SALT"]),
        ),
        (
            "{'k': [steps.a.output, {'n': steps.b.output}]}[steps.c.output.key]",
            refs(&["a", "b", "c"], &[]),
        ),
        (
            "steps.a.output.list.exists(x, x == steps.b.output.v) ? secrets.A : secrets.B",
            refs(&["a", "b"], &["A", "B"]),
        ),
        ("size(steps.a.output.name.lowerAscii())", refs(&["a"], &[])),
        ("input.steps.a", refs(&[], &[])),
        ("[{'a': 1}].map(steps, steps.a)", refs(&[], &[])),
        ("[1].map(x, steps.a.output)", refs(&["a"], &[])),
    ];
    for (source, expected) in cases {
        let expression = Expression::compile(source).unwrap();
        assert_eq!(expression.references(), expected, "{source}");
    }
}

#[test]
fn extraction_flags_reads_without_a_literal_name() {
    let references = Expression::compile("steps[input.step].output")
        .unwrap()
        .references()
        .clone();
    assert!(references.dynamic_steps);
    assert!(references.steps.is_empty());

    let references = Expression::compile("size(secrets) > 0 && steps.a.output")
        .unwrap()
        .references()
        .clone();
    assert!(references.dynamic_secrets);
    assert!(!references.dynamic_steps);
    assert_eq!(references.steps, refs(&["a"], &[]).steps);
}

#[test]
fn template_extraction_unions_its_segments() {
    let template = Template::parse(
        "Bearer {{ secrets.TOKEN }} for {{ steps.user.output.id }}/{{ steps.user.output.org }} {{ steps.other.output }}",
    )
    .unwrap();
    assert_eq!(template.references(), &refs(&["other", "user"], &["TOKEN"]));
}

#[test]
fn pathological_expressions_stop_at_the_cost_limit() {
    let range = format!(
        "[{}]",
        (0..100)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    // 100^4 = 100 million iterations without a limit.
    let source = format!(
        "{range}.map(a, {range}.map(b, {range}.map(c, {range}.filter(d, a + b + c + d > 0))))"
    );
    let error = Expression::compile(&source).unwrap().evaluate(&scope());
    assert_eq!(
        error,
        Err(EvalError::CostLimitExceeded {
            limit: Limits::default().max_cost
        })
    );

    for source in [
        format!("{range}.all(a, {range}.all(b, {range}.all(c, {range}.all(d, true))))"),
        format!(
            "{range}.exists(a, {range}.exists(b, {range}.exists(c, {range}.exists(d, false))))"
        ),
        format!(
            "{range}.exists_one(a, {range}.exists_one(b, {range}.exists_one(c, {range}.exists_one(d, true))))"
        ),
        // `||` absorbs an error beside a `true`, but the evaluation still fails.
        format!("{range}.map(a, {range}.map(b, {range}.map(c, {range}.map(d, d)))) == [] || true"),
    ] {
        let result = Expression::compile(&source).unwrap().evaluate(&scope());
        assert!(
            matches!(result, Err(EvalError::CostLimitExceeded { .. })),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn cost_counts_the_size_of_produced_values() {
    let limits = Limits {
        max_cost: 1_000,
        ..Limits::default()
    };
    let big = json!((0..500).collect::<Vec<_>>());
    let scope = Scope::new().with_input(&json!({ "big": big }));
    // 10 iterations, each copying a 500-element list.
    let source = "[1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map(x, input.big)";
    let result = Expression::compile_with(source, &limits)
        .unwrap()
        .evaluate(&scope);
    assert!(matches!(
        result,
        Err(EvalError::CostLimitExceeded { limit: 1_000 })
    ));

    let source = "[1].map(x, input.big)";
    let result = Expression::compile_with(source, &limits)
        .unwrap()
        .evaluate(&scope);
    assert!(result.is_ok());
}

#[test]
fn metering_keeps_macro_semantics() {
    assert_eq!(eval("[1, 2, 3].map(x, x * 2)"), Ok(json!([2, 4, 6])));
    assert_eq!(eval("[1, 2, 3].map(x, x > 1, x * 10)"), Ok(json!([20, 30])));
    assert_eq!(eval("[1, 2, 3].filter(x, x != 2)"), Ok(json!([1, 3])));
    assert_eq!(eval("[1, 2, 3].all(x, x > 0)"), Ok(json!(true)));
    assert_eq!(eval("[1, 2, 3].exists(x, x == 3)"), Ok(json!(true)));
    assert_eq!(eval("[1, 2, 3].exists_one(x, x > 2)"), Ok(json!(true)));
    assert_eq!(eval("{'a': 1, 'b': 2}.all(k, k != 'c')"), Ok(json!(true)));
    // `exists` absorbs an error when another element decides the result.
    assert_eq!(eval("[0, 1].exists(x, 1 / x == 1)"), Ok(json!(true)));
    assert!(eval("[0, 2].all(x, 1 / x == 1)").is_ok());
    assert!(matches!(
        eval("[0].all(x, 1 / x == 1)"),
        Err(EvalError::Failed { .. })
    ));
}

#[test]
fn missing_scope_keys_are_clear_errors() {
    assert_eq!(
        eval("steps.missing.output"),
        Err(EvalError::NoSuchKey {
            key: "missing".into()
        })
    );
    assert_eq!(
        eval("input.user.age > 3"),
        Err(EvalError::NoSuchKey { key: "age".into() })
    );
    assert_eq!(
        eval("item.id"),
        Err(EvalError::UnknownVariable {
            name: "item".into()
        })
    );
    assert_eq!(
        eval("secrets.TOKEN"),
        Err(EvalError::UnknownVariable {
            name: "secrets".into()
        })
    );
    assert!(matches!(
        eval("input.tags[10]"),
        Err(EvalError::Failed { .. })
    ));
    assert!(matches!(eval("1 / 0"), Err(EvalError::Failed { .. })));
    assert!(matches!(
        eval("input.user + 1"),
        Err(EvalError::Failed { .. })
    ));
    assert!(matches!(
        eval("unknown_function(1)"),
        Err(EvalError::UnknownVariable { .. })
    ));
}

#[test]
fn foreach_and_catch_variables() {
    let scope = scope()
        .with_item(&json!({ "id": 7 }), 2)
        .with_error(&json!({ "code": "timeout" }));
    let expression = Expression::compile("[item.id, index, error.code]").unwrap();
    assert_eq!(expression.evaluate(&scope), Ok(json!([7, 2, "timeout"])));
}

#[test]
fn secrets_are_readable_but_never_leak_into_errors() {
    let scope = scope().with_secret("TOKEN", "s3cr3t-value");
    let template = Template::parse("Bearer {{ secrets.TOKEN }}").unwrap();
    assert_eq!(template.evaluate(&scope), Ok(json!("Bearer s3cr3t-value")));

    for source in [
        "input.user[secrets.TOKEN]",
        "{'a': 1}[secrets.TOKEN]",
        "secrets.TOKEN < 1",
        "secrets.TOKEN + 1",
        "int(secrets.TOKEN)",
        "secrets.TOKEN.substring(100)",
        "timestamp(secrets.TOKEN)",
        "{secrets.TOKEN: 1, secrets.TOKEN: 2}",
    ] {
        let error = Expression::compile(source)
            .unwrap()
            .evaluate(&scope)
            .unwrap_err();
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains("s3cr3t"), "{source}: {shown}");
    }
    assert!(!format!("{scope:?}").contains("s3cr3t"));
}

#[test]
fn compiled_expressions_are_reusable_across_scopes() {
    let expression = Expression::compile("steps.a.output.count * 2").unwrap();
    for count in [1, 2, 3] {
        let scope = Scope::new().with_step_output("a", &json!({ "count": count }));
        assert_eq!(expression.evaluate(&scope), Ok(json!(count * 2)));
    }
}
