//! `GET /v1/schema`: what an agent needs to author a valid Definition without guessing.

use axum::extract::State;
use bia_core::action::{ActionExample, CatalogueEntry, Effect};
use bia_core::dsl::{definition_schema, queue_resource_schema};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::{ApiState, Json};

/// The DSL's JSON Schemas and the Action catalogue.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SchemaDocument {
    /// The JSON Schema (draft 2020-12) a Definition must satisfy. Passing it is the first
    /// layer of validation; Diagnostics report the rest.
    #[schema(value_type = Object)]
    definition: Value,
    /// The JSON Schema of a `kind: queue` resource file.
    #[schema(value_type = Object)]
    queue: Value,
    /// Every Action this server knows, disabled ones included, ordered by name.
    actions: Vec<ActionEntry>,
}

/// An Action a Step can name in `action`.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActionEntry {
    /// `category.operation`, e.g. `http.request`.
    name: String,
    category: String,
    description: String,
    effect: ActionEffect,
    /// Disabled unless enabled in server config, and only publishable with an `admin` Token.
    dangerous: bool,
    /// A Version that uses a disabled Action cannot be published.
    enabled: bool,
    /// The JSON Schema a Step's `params` must satisfy once its Templates are resolved.
    #[schema(value_type = Object)]
    param_schema: Value,
    /// The JSON Schema of the Step's Output, read by later Steps as `steps.<id>.output`.
    #[schema(value_type = Object)]
    output_schema: Value,
    /// Example calls, also the source of Mocks in a Dry Run.
    examples: Vec<ActionExampleEntry>,
}

/// Whether an Action can change anything outside the Run. A Dry Run executes `pure` Actions
/// and gives `side_effecting` ones a Mock.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActionEffect {
    Pure,
    SideEffecting,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActionExampleEntry {
    description: String,
    #[schema(value_type = Object)]
    params: Value,
    output: Value,
}

/// The DSL JSON Schema and every Action with its param and output schemas and examples.
#[utoipa::path(
    get,
    path = "/v1/schema",
    tag = "schema",
    responses((status = OK, body = SchemaDocument))
)]
pub(crate) async fn get_schema(State(state): State<ApiState>) -> Json<SchemaDocument> {
    Json(SchemaDocument {
        definition: definition_schema(),
        queue: queue_resource_schema(),
        actions: state
            .actions
            .catalogue()
            .into_iter()
            .map(ActionEntry::from)
            .collect(),
    })
}

impl From<CatalogueEntry> for ActionEntry {
    fn from(entry: CatalogueEntry) -> Self {
        let descriptor = entry.descriptor;
        Self {
            category: descriptor.category().to_owned(),
            name: descriptor.name.as_str().to_owned(),
            description: descriptor.description,
            effect: match descriptor.effect {
                Effect::Pure => ActionEffect::Pure,
                Effect::SideEffecting => ActionEffect::SideEffecting,
            },
            dangerous: descriptor.dangerous,
            enabled: entry.enabled,
            param_schema: descriptor.param_schema.to_value(),
            output_schema: descriptor.output_schema.to_value(),
            examples: descriptor
                .examples
                .into_iter()
                .map(ActionExampleEntry::from)
                .collect(),
        }
    }
}

impl From<ActionExample> for ActionExampleEntry {
    fn from(example: ActionExample) -> Self {
        Self {
            description: example.description,
            params: Value::Object(example.params),
            output: example.output,
        }
    }
}
