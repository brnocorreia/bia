//! The committed `openapi.json` is the document the handlers generate, and the one served.
//! Rewrite it with `just openapi`.

#![allow(clippy::unwrap_used)]

use std::path::Path;

use serde_json::Value;
use sqlx::PgPool;

mod common;

use common::TestServer;

const COMMITTED: &str = include_str!("../openapi.json");

#[test]
fn the_committed_openapi_document_is_up_to_date() {
    let generated = bia_server::openapi().to_pretty_json().unwrap();
    if std::env::var_os("BIA_UPDATE_OPENAPI").is_some() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
        std::fs::write(path, format!("{generated}\n")).unwrap();
        return;
    }
    assert!(
        COMMITTED.trim_end() == generated,
        "crates/bia-server/openapi.json is out of date; run `just openapi` and commit it"
    );
}

#[test]
fn the_error_body_documents_every_field_it_serialises() {
    let document: Value = serde_json::from_str(COMMITTED).unwrap();
    let schemas = &document["components"]["schemas"];
    let diagnostic = bia_core::validation::Diagnostic {
        path: "/steps/0".into(),
        code: bia_core::validation::Code::UnknownAction,
        severity: bia_core::validation::Severity::Error,
        message: "unknown".into(),
        suggestion: Some("test.echo".into()),
    };
    let serialised = serde_json::to_value(&diagnostic).unwrap();
    let mut fields: Vec<_> = serialised.as_object().unwrap().keys().cloned().collect();
    let mut documented: Vec<_> = schemas["Diagnostic"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    fields.sort();
    documented.sort();
    assert_eq!(fields, documented);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_server_serves_the_committed_document(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let served = reqwest::get(server.url("/openapi.json"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(served, COMMITTED.trim_end());
}

/// Generators such as `openapi-typescript` reject a document with a dangling `$ref` or a path
/// parameter its path template lacks, and the UI types are generated from this file.
#[test]
fn the_committed_document_is_self_consistent() {
    let document: Value = serde_json::from_str(COMMITTED).unwrap();
    for (path, operations) in document["paths"].as_object().unwrap() {
        for (method, operation) in operations.as_object().unwrap() {
            for parameter in operation["parameters"].as_array().into_iter().flatten() {
                if parameter["in"] == "path" {
                    let name = parameter["name"].as_str().unwrap();
                    assert!(
                        path.contains(&format!("{{{name}}}")),
                        "{method} {path}: `{name}` is a path parameter the path lacks"
                    );
                }
            }
        }
    }
    let mut refs = Vec::new();
    collect_refs(&document, &mut refs);
    for reference in refs {
        let pointer = reference.trim_start_matches('#');
        assert!(
            document.pointer(pointer).is_some(),
            "dangling reference {reference}"
        );
    }
}

fn collect_refs(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                match (key.as_str(), inner) {
                    ("$ref", Value::String(target)) => found.push(target.clone()),
                    _ => collect_refs(inner, found),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_refs(item, found)),
        _ => {}
    }
}
