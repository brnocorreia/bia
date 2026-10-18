//! Fixtures shared by the store tests.

#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use bia_core::dsl::{Definition, WorkflowId};
use bia_store::runs::{Checkpoint, NewRun, Run};
use bia_store::versions::{Version, VersionNumber};
use bia_store::{runs, versions, workflows};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::json;
use sqlx::PgConnection;

/// A fixed instant tests advance from, standing in for the Clock.
pub fn t0() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

pub fn at(seconds: i64) -> DateTime<Utc> {
    t0() + TimeDelta::seconds(seconds)
}

pub fn workflow_id(id: &str) -> WorkflowId {
    WorkflowId::new(id).unwrap()
}

pub fn number(n: i32) -> VersionNumber {
    VersionNumber::new(n).unwrap()
}

/// A valid Definition of `workflow` whose only Step has id `step`.
pub fn definition(workflow: &str, step: &str) -> Definition {
    Definition::from_value(json!({
        "schema_version": 1,
        "id": workflow,
        "steps": [{ "id": step, "action": "flow.succeed" }],
    }))
    .unwrap()
}

/// Creates `workflow` with one Published Version.
pub async fn published_workflow(conn: &mut PgConnection, workflow: &str) -> Version {
    workflows::create(conn, &workflow_id(workflow))
        .await
        .unwrap();
    let draft = versions::create_draft(conn, &definition(workflow, "first"))
        .await
        .unwrap();
    versions::publish(conn, &draft.workflow_id, draft.number, t0())
        .await
        .unwrap()
}

/// Starts a Run on `version` at `when`.
pub async fn start_run(conn: &mut PgConnection, version: &Version, when: DateTime<Utc>) -> Run {
    let run = NewRun {
        workflow_id: version.workflow_id.clone(),
        version: version.number,
        input: json!({ "user": 42 }),
        trigger: json!({ "type": "manual" }),
        checkpoint: Checkpoint {
            cursor: vec![],
            state: json!({}),
        },
    };
    runs::create(conn, &run, when).await.unwrap()
}
