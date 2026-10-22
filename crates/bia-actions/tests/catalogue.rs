//! Every builtin Action's descriptor, checked against itself.

use bia_core::action::ActionRegistry;
use serde_json::Value;

/// Holds for every Action registered by `builtin_registry`, so new Actions are covered too.
#[test]
fn examples_satisfy_their_own_param_and_output_schemas() {
    let registry = bia_actions::builtin_registry().unwrap();
    let catalogue = registry.catalogue();
    assert!(!catalogue.is_empty());
    for entry in catalogue {
        let descriptor = entry.descriptor;
        let name = &descriptor.name;
        assert!(!descriptor.examples.is_empty(), "`{name}` has no examples");
        let params = jsonschema::validator_for(descriptor.param_schema.as_value())
            .unwrap_or_else(|e| panic!("`{name}` param schema: {e}"));
        let output = jsonschema::validator_for(descriptor.output_schema.as_value())
            .unwrap_or_else(|e| panic!("`{name}` output schema: {e}"));
        for example in &descriptor.examples {
            let example_params = Value::Object(example.params.clone());
            let errors: Vec<String> = params
                .iter_errors(&example_params)
                .map(|e| format!("{}: {e}", e.instance_path()))
                .collect();
            assert!(
                errors.is_empty(),
                "`{name}` example {:?} params: {errors:?}",
                example.description
            );
            let errors: Vec<String> = output
                .iter_errors(&example.output)
                .map(|e| format!("{}: {e}", e.instance_path()))
                .collect();
            assert!(
                errors.is_empty(),
                "`{name}` example {:?} output: {errors:?}",
                example.description
            );
        }
    }
}

#[test]
fn builtin_actions_are_enabled() {
    let registry = bia_actions::builtin_registry().unwrap();
    let names: Vec<(String, bool)> = registry
        .catalogue()
        .into_iter()
        .map(|entry| (entry.descriptor.name.as_str().to_owned(), entry.enabled))
        .collect();
    assert_eq!(
        names,
        [
            ("http.request".to_owned(), true),
            ("transform.expr".to_owned(), true)
        ]
    );
}
