//! The tool table: name, agent-facing description, input schema, and the one API request a
//! call becomes. Names, descriptions and argument shapes are a contract with agents (ADR-0048),
//! snapshot-tested so a change is reviewed.

use axum::http::Method;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Map, Value, json};

/// The API request a tool call becomes.
#[derive(Debug)]
pub(super) struct Call {
    pub method: Method,
    /// Path and query, already percent-encoded.
    pub uri: String,
    pub body: Option<Value>,
    /// Keep only this field of the response.
    pub only: Option<&'static str>,
}

type Args = Map<String, Value>;

pub(super) struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub read_only: bool,
    pub schema: fn() -> Value,
    pub build: fn(&Args) -> Result<Call, String>,
}

pub(super) static TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "get_schema",
        description: "Returns the JSON Schema a Workflow Definition must satisfy, the JSON \
Schema of a Queue resource, and the catalogue of Actions (see `list_actions`). Call it before \
writing a Definition. Maps to `GET /v1/schema`.",
        read_only: true,
        schema: || object(json!({}), &[]),
        build: |_| Ok(get("/v1/schema".into(), None)),
    },
    ToolDef {
        name: "list_actions",
        description: "Lists every Action a Step can call: name, description, parameter schema, \
output schema, examples, whether it is `pure` or `side_effecting`, and whether it is dangerous \
or disabled. Use it to pick Actions and to shape their `params`. Returns `{ actions }`, the \
`actions` of `GET /v1/schema`.",
        read_only: true,
        schema: || object(json!({}), &[]),
        build: |_| Ok(get("/v1/schema".into(), Some("actions"))),
    },
    ToolDef {
        name: "list_workflows",
        description: "Lists every Workflow with its Active Version, if any. Maps to \
`GET /v1/workflows`.",
        read_only: true,
        schema: || object(json!({}), &[]),
        build: |_| Ok(get("/v1/workflows".into(), None)),
    },
    ToolDef {
        name: "list_secrets",
        description: "Lists the names of the Secrets the server holds, with when each was \
created and last set. Reference one in a Step's `params` as `{{ secrets.NAME }}`. Values are \
never returned by any tool, and no tool sets or deletes a Secret. Maps to `GET /v1/secrets`.",
        read_only: true,
        schema: || object(json!({}), &[]),
        build: |_| Ok(get("/v1/secrets".into(), None)),
    },
    ToolDef {
        name: "list_queues",
        description: "Lists every Queue with its settings: `max_retries`, `visibility_timeout`, \
`batch_size` and `dead_letter`. A Step publishes to one with `queue.publish` or \
`queue.publish_batch`, and a `queue` Trigger consumes one; both must name an existing Queue, \
since Queues are never created on first use and no tool creates one (ask a human with an \
`admin` Token). Maps to `GET /v1/queues`.",
        read_only: true,
        schema: || object(json!({}), &[]),
        build: |_| Ok(get("/v1/queues".into(), None)),
    },
    ToolDef {
        name: "get_workflow",
        description: "Shows one Workflow and which Version is Active. Fails with \
`workflow_not_found` when it does not exist. Maps to `GET /v1/workflows/{workflow_id}`.",
        read_only: true,
        schema: || {
            object(
                json!({ "workflow_id": shared("workflow_id") }),
                &["workflow_id"],
            )
        },
        build: |a| {
            Ok(get(
                format!("/v1/workflows/{}", seg(a, "workflow_id")?),
                None,
            ))
        },
    },
    ToolDef {
        name: "list_versions",
        description: "Lists a Workflow's Versions, oldest first: number, Draft or Published, \
whether it is the Active Version. Maps to `GET /v1/workflows/{workflow_id}/versions`.",
        read_only: true,
        schema: || {
            object(
                json!({ "workflow_id": shared("workflow_id") }),
                &["workflow_id"],
            )
        },
        build: |a| {
            let id = seg(a, "workflow_id")?;
            Ok(get(format!("/v1/workflows/{id}/versions"), None))
        },
    },
    ToolDef {
        name: "get_version",
        description: "Shows one Version of a Workflow with its Definition as stored. Use it to \
read the Definition you are about to change. Maps to \
`GET /v1/workflows/{workflow_id}/versions/{number}`.",
        read_only: true,
        schema: || {
            object(
                json!({ "workflow_id": shared("workflow_id"), "number": shared("number") }),
                &["workflow_id", "number"],
            )
        },
        build: |a| {
            let (id, number) = (seg(a, "workflow_id")?, int(a, "number")?);
            Ok(get(format!("/v1/workflows/{id}/versions/{number}"), None))
        },
    },
    ToolDef {
        name: "validate_workflow",
        description: "Checks a Definition without saving it and returns `{ valid, diagnostics }`. \
Every Diagnostic has a `path` into the Definition, a stable `code`, a `message` and usually a \
`suggestion`: fix the errors, then validate again. Give either `definition` (a Definition that \
need not be stored) or `workflow_id` and `number` (a stored Version). `target` is `draft` \
(default: is it complete enough to publish later?) or `publish` (is it ready to publish now?). \
Maps to `POST /v1/validate` or `POST /v1/workflows/{workflow_id}/versions/{number}/validate`.",
        read_only: true,
        schema: || {
            object(
                json!({
                    "definition": shared("definition"),
                    "workflow_id": shared("workflow_id"),
                    "number": shared("number"),
                    "target": { "type": "string", "enum": ["draft", "publish"], "default": "draft" },
                }),
                &[],
            )
        },
        build: |a| {
            let query = query(&[("target", opt_text(a, "target")?)]);
            match target(a)? {
                Target::Definition(definition) => {
                    Ok(post(format!("/v1/validate{query}"), Some(definition)))
                }
                Target::Version { id, number } => Ok(post(
                    format!("/v1/workflows/{id}/versions/{number}/validate{query}"),
                    None,
                )),
            }
        },
    },
    ToolDef {
        name: "create_draft",
        description: "Saves a Definition as a new Draft Version of the Workflow named by its \
`id`, creating the Workflow if it is new. Semantic errors do not stop the save: they come back \
as `diagnostics`, so fix them and call `replace_draft`. A Draft starts no Runs. Publishing and \
activating it need an admin and are not available here: tell the human which Version to \
publish. Maps to `POST /v1/workflows/{workflow_id}/versions`.",
        read_only: false,
        schema: || {
            object(
                json!({ "workflow_id": shared("workflow_id"), "definition": shared("definition") }),
                &["workflow_id", "definition"],
            )
        },
        build: |a| {
            let id = seg(a, "workflow_id")?;
            Ok(post(
                format!("/v1/workflows/{id}/versions"),
                Some(definition(a)?),
            ))
        },
    },
    ToolDef {
        name: "replace_draft",
        description: "Replaces the Definition of a Draft Version. A Published Version is frozen \
and fails with `version_published`: create a new Draft instead. Semantic errors come back as \
`diagnostics`. Maps to `PUT /v1/workflows/{workflow_id}/versions/{number}`.",
        read_only: false,
        schema: || {
            object(
                json!({ "workflow_id": shared("workflow_id"), "number": shared("number"), "definition": shared("definition") }),
                &["workflow_id", "number", "definition"],
            )
        },
        build: |a| {
            let (id, number) = (seg(a, "workflow_id")?, int(a, "number")?);
            Ok(Call {
                method: Method::PUT,
                uri: format!("/v1/workflows/{id}/versions/{number}"),
                body: Some(definition(a)?),
                only: None,
            })
        },
    },
    ToolDef {
        name: "dry_run",
        description: "Runs a Definition in memory, nothing saved, no Run created, and returns \
the Timeline it would produce plus `status` (`succeeded` or `failed`). Waits finish at once. \
Actions that are `pure` run for real; `side_effecting` ones return their first example's \
Output unless you Mock them in `mocks`, a map from Step id to exactly one of \
`{ output }`, `{ error: { class: retryable|fatal, message } }` or `{ attempts: [...] }` (the \
Nth Attempt gets the Nth outcome, the last repeats). A Step with neither a Mock nor an \
example fails with `no_mock`. Give either `definition` or `workflow_id` and `number`. Maps to \
`POST /v1/dry-run` or `POST /v1/workflows/{workflow_id}/versions/{number}/dry-run`.",
        read_only: true,
        schema: || {
            object(
                json!({
                    "definition": shared("definition"),
                    "workflow_id": shared("workflow_id"),
                    "number": shared("number"),
                    "input": { "description": "The Run input. Defaults to `{}`." },
                    "mocks": { "type": "object", "description": "Mocks by Step id." },
                }),
                &[],
            )
        },
        build: |a| {
            let mut body = Map::new();
            for key in ["input", "mocks"] {
                if let Some(value) = a.get(key) {
                    body.insert(key.into(), value.clone());
                }
            }
            match target(a)? {
                Target::Definition(definition) => {
                    body.insert("definition".into(), definition);
                    Ok(post("/v1/dry-run".into(), Some(body.into())))
                }
                Target::Version { id, number } => Ok(post(
                    format!("/v1/workflows/{id}/versions/{number}/dry-run"),
                    Some(body.into()),
                )),
            }
        },
    },
    ToolDef {
        name: "diff_versions",
        description: "Shows what changed between two Versions of a Workflow as a JSON Patch \
from `from` to `to`. Maps to `GET /v1/workflows/{workflow_id}/diff?from=&to=`.",
        read_only: true,
        schema: || {
            object(
                json!({
                    "workflow_id": shared("workflow_id"),
                    "from": { "type": "integer", "minimum": 1, "description": "The Version number to diff from." },
                    "to": { "type": "integer", "minimum": 1, "description": "The Version number to diff to." },
                }),
                &["workflow_id", "from", "to"],
            )
        },
        build: |a| {
            let id = seg(a, "workflow_id")?;
            let query = query(&[
                ("from", Some(int(a, "from")?.to_string())),
                ("to", Some(int(a, "to")?.to_string())),
            ]);
            Ok(get(format!("/v1/workflows/{id}/diff{query}"), None))
        },
    },
    ToolDef {
        name: "trigger_run",
        description: "Starts a Run of the Workflow's Active Version from a manual Trigger and \
returns it as `pending`; follow it with `get_run`. Fails with `no_active_version` while no \
Version is active (an admin activates one), and with `invalid_run_input` when `input` does not \
match the Version's `input_schema`. This has real effects: use `dry_run` to try a Definition. \
Maps to `POST /v1/workflows/{workflow_id}/runs`.",
        read_only: false,
        schema: || {
            object(
                json!({
                    "workflow_id": shared("workflow_id"),
                    "input": { "description": "The Run input. Defaults to `{}`." },
                    "trigger_id": { "type": "string", "description": "The manual Trigger to start from. Defaults to the first one." },
                }),
                &["workflow_id"],
            )
        },
        build: |a| {
            let id = seg(a, "workflow_id")?;
            let mut body = Map::new();
            for key in ["input", "trigger_id"] {
                if let Some(value) = a.get(key) {
                    body.insert(key.into(), value.clone());
                }
            }
            Ok(post(format!("/v1/workflows/{id}/runs"), Some(body.into())))
        },
    },
    ToolDef {
        name: "get_run",
        description: "Shows a Run: its status, input, output, error and Timeline (one entry per \
Attempt of each Step, with the Output or error). Poll it until the status is `succeeded`, \
`failed` or `cancelled`; `pending`, `running`, `sleeping` and `waiting` are not final. Maps to \
`GET /v1/runs/{run_id}`.",
        read_only: true,
        schema: || object(json!({ "run_id": shared("run_id") }), &["run_id"]),
        build: |a| Ok(get(format!("/v1/runs/{}", seg(a, "run_id")?), None)),
    },
    ToolDef {
        name: "list_runs",
        description: "Lists Runs, newest first, optionally of one Workflow or status or created \
within a time range. A page ends with `next_after`: pass it as `after` for the next page. Maps \
to `GET /v1/runs`.",
        read_only: true,
        schema: || {
            object(
                json!({
                    "workflow_id": { "type": "string", "description": "Only Runs of this Workflow." },
                    "status": { "type": "string", "enum": ["pending", "running", "sleeping", "waiting", "succeeded", "failed", "cancelled"] },
                    "created_from": { "type": "string", "format": "date-time", "description": "Only Runs created at or after this time (RFC 3339)." },
                    "created_to": { "type": "string", "format": "date-time", "description": "Only Runs created before this time (RFC 3339)." },
                    "after": { "type": "string", "format": "uuid", "description": "A previous page's `next_after`." },
                    "limit": { "type": "integer", "minimum": 1 },
                }),
                &[],
            )
        },
        build: |a| Ok(get(format!("/v1/runs{}", run_filters(a, true)?), None)),
    },
    ToolDef {
        name: "list_failures",
        description: "Lists failed Runs, newest first, with the Step that failed and why: the \
place to start when something is broken. Paged like `list_runs`. Maps to \
`GET /v1/runs/failures`.",
        read_only: true,
        schema: || {
            object(
                json!({
                    "workflow_id": { "type": "string", "description": "Only failures of this Workflow." },
                    "created_from": { "type": "string", "format": "date-time" },
                    "created_to": { "type": "string", "format": "date-time" },
                    "after": { "type": "string", "format": "uuid", "description": "A previous page's `next_after`." },
                    "limit": { "type": "integer", "minimum": 1 },
                }),
                &[],
            )
        },
        build: |a| {
            Ok(get(
                format!("/v1/runs/failures{}", run_filters(a, false)?),
                None,
            ))
        },
    },
    ToolDef {
        name: "retry_run",
        description: "Starts a new Run on the same Version and input as a finished one, \
linked to the original. Use it to run again from the start; use `resume_run` to continue a \
failed Run from the Step that failed. Maps to `POST /v1/runs/{run_id}/retry`.",
        read_only: false,
        schema: || object(json!({ "run_id": shared("run_id") }), &["run_id"]),
        build: |a| Ok(post(format!("/v1/runs/{}/retry", seg(a, "run_id")?), None)),
    },
    ToolDef {
        name: "resume_run",
        description: "Continues a failed Run from the Step that failed, keeping the Steps that \
already succeeded. Fix the cause first (for instance a Secret or an upstream service). Maps to \
`POST /v1/runs/{run_id}/resume`.",
        read_only: false,
        schema: || object(json!({ "run_id": shared("run_id") }), &["run_id"]),
        build: |a| Ok(post(format!("/v1/runs/{}/resume", seg(a, "run_id")?), None)),
    },
    ToolDef {
        name: "cancel_run",
        description: "Cancels a Run that is not finished: the Step in flight stops and no \
later Step runs. Returns at once; the Run ends `cancelled` shortly after. Maps to \
`POST /v1/runs/{run_id}/cancel`.",
        read_only: false,
        schema: || object(json!({ "run_id": shared("run_id") }), &["run_id"]),
        build: |a| Ok(post(format!("/v1/runs/{}/cancel", seg(a, "run_id")?), None)),
    },
];

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// The schema of an argument several tools share.
fn shared(name: &str) -> Value {
    match name {
        "workflow_id" => json!({ "type": "string", "description": "The Workflow's id." }),
        "number" => {
            json!({ "type": "integer", "minimum": 1, "description": "The Version number." })
        }
        "run_id" => json!({ "type": "string", "format": "uuid", "description": "The Run's id." }),
        "definition" => json!({
            "type": "object",
            "description": "A Workflow Definition as JSON; its shape is `definition` in `get_schema`.",
        }),
        _ => unreachable!("`{name}` has no shared schema"),
    }
}

fn get(uri: String, only: Option<&'static str>) -> Call {
    Call {
        method: Method::GET,
        uri,
        body: None,
        only,
    }
}

fn post(uri: String, body: Option<Value>) -> Call {
    Call {
        method: Method::POST,
        uri,
        body,
        only: None,
    }
}

/// A path segment from a string argument.
fn seg(args: &Args, name: &str) -> Result<String, String> {
    let value = match args.get(name) {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => {
            return Err(format!(
                "`{name}` is required and must be a non-empty string"
            ));
        }
    };
    Ok(utf8_percent_encode(value, NON_ALPHANUMERIC).to_string())
}

fn int(args: &Args, name: &str) -> Result<i32, String> {
    args.get(name)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| *value >= 1)
        .ok_or_else(|| format!("`{name}` is required and must be a positive integer"))
}

fn opt_text(args: &Args, name: &str) -> Result<Option<String>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Number(value)) => Ok(Some(value.to_string())),
        Some(_) => Err(format!("`{name}` must be a string or a number")),
    }
}

fn definition(args: &Args) -> Result<Value, String> {
    match args.get("definition") {
        Some(definition) if definition.is_object() => Ok(definition.clone()),
        _ => Err("`definition` is required and must be an object".into()),
    }
}

enum Target {
    Definition(Value),
    Version { id: String, number: i32 },
}

/// What a tool that takes a Definition or a stored Version works on; exactly one is given.
fn target(args: &Args) -> Result<Target, String> {
    match (
        args.contains_key("definition"),
        args.contains_key("workflow_id"),
    ) {
        (true, false) if !args.contains_key("number") => definition(args).map(Target::Definition),
        (false, true) => Ok(Target::Version {
            id: seg(args, "workflow_id")?,
            number: int(args, "number")?,
        }),
        _ => Err("give either `definition`, or `workflow_id` and `number`, not both".into()),
    }
}

fn query(pairs: &[(&str, Option<String>)]) -> String {
    let encoded: Vec<String> = pairs
        .iter()
        .filter_map(|(key, value)| {
            let value = value.as_deref()?;
            Some(format!(
                "{key}={}",
                utf8_percent_encode(value, NON_ALPHANUMERIC)
            ))
        })
        .collect();
    if encoded.is_empty() {
        String::new()
    } else {
        format!("?{}", encoded.join("&"))
    }
}

fn run_filters(args: &Args, with_status: bool) -> Result<String, String> {
    let status = if with_status {
        opt_text(args, "status")?
    } else {
        None
    };
    Ok(query(&[
        ("workflow_id", opt_text(args, "workflow_id")?),
        ("status", status),
        ("created_from", opt_text(args, "created_from")?),
        ("created_to", opt_text(args, "created_to")?),
        ("after", opt_text(args, "after")?),
        ("limit", opt_text(args, "limit")?),
    ]))
}
