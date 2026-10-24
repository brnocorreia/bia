//! `GET /v1/schema` gives an agent the DSL JSON Schema and the whole Action catalogue.

#![allow(clippy::unwrap_used)]

use bia_core::dsl::{definition_schema, queue_resource_schema};
use serde_json::json;
use sqlx::PgPool;

mod common;

use common::TestServer;

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_schema_lists_the_dsl_schemas_and_every_registered_action(pool: PgPool) {
    let server = TestServer::start(pool).await;

    let schema = server.client.schema().await.unwrap();

    assert_eq!(schema.definition, definition_schema());
    assert_eq!(schema.queue, queue_resource_schema());
    let names: Vec<_> = schema.actions.iter().map(|a| a.name.clone()).collect();
    assert_eq!(names, common::catalogue_names());

    let echo = schema
        .actions
        .iter()
        .find(|a| a.name == "test.echo")
        .unwrap();
    assert_eq!(echo.category, "test");
    assert_eq!(echo.description, "Returns its Params.");
    assert_eq!(echo.param_schema["required"], json!(["greeting"]));
    assert_eq!(echo.output_schema, json!({ "type": "object" }));
    assert_eq!(echo.examples.len(), 1);
    assert_eq!(echo.examples[0].params, json!({ "greeting": "hi" }));
    assert_eq!(echo.examples[0].output, json!({ "greeting": "hi" }));
    assert!(echo.enabled);

    let shell = schema
        .actions
        .iter()
        .find(|a| a.name == "test.shell")
        .unwrap();
    assert!(shell.dangerous);
    assert!(!shell.enabled, "disabled Actions are listed too");

    let sleep = schema
        .actions
        .iter()
        .find(|a| a.name == "flow.sleep")
        .unwrap();
    assert_eq!(sleep.effect, "pure");
    assert_eq!(shell.effect, "side_effecting");
}
