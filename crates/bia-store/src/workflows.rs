//! Workflows: stable identities that own Versions and point at one Active Version.

use bia_core::dsl::WorkflowId;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;

use crate::StoreError;
use crate::versions::{self, VersionNumber};

/// A Workflow and its Active Version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub id: WorkflowId,
    /// The Published Version Triggers start Runs on, once one has been activated.
    pub active_version: Option<VersionNumber>,
    pub created_at: DateTime<Utc>,
}

/// A `workflows` row as read back from Postgres.
struct WorkflowRow {
    id: String,
    active_version: Option<i32>,
    created_at: DateTime<Utc>,
}

impl TryFrom<WorkflowRow> for Workflow {
    type Error = StoreError;

    fn try_from(row: WorkflowRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: WorkflowId::new(row.id).map_err(|err| StoreError::invalid("workflow id", err))?,
            active_version: row.active_version.map(VersionNumber::from_db).transpose()?,
            created_at: row.created_at,
        })
    }
}

/// Creates a Workflow with no Versions.
pub async fn create(conn: &mut PgConnection, id: &WorkflowId) -> Result<Workflow, StoreError> {
    let row = sqlx::query_as!(
        WorkflowRow,
        r#"
        INSERT INTO workflows (id) VALUES ($1)
        ON CONFLICT (id) DO NOTHING
        RETURNING id, active_version, created_at
        "#,
        id.as_str(),
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| StoreError::WorkflowExists(id.clone()))?;

    Workflow::try_from(row)
}

/// Reads one Workflow.
pub async fn get(conn: &mut PgConnection, id: &WorkflowId) -> Result<Option<Workflow>, StoreError> {
    let row = sqlx::query_as!(
        WorkflowRow,
        "SELECT id, active_version, created_at FROM workflows WHERE id = $1",
        id.as_str(),
    )
    .fetch_optional(&mut *conn)
    .await?;

    row.map(Workflow::try_from).transpose()
}

/// Lists every Workflow by id.
pub async fn list(conn: &mut PgConnection) -> Result<Vec<Workflow>, StoreError> {
    let rows = sqlx::query_as!(
        WorkflowRow,
        "SELECT id, active_version, created_at FROM workflows ORDER BY id",
    )
    .fetch_all(&mut *conn)
    .await?;

    rows.into_iter().map(Workflow::try_from).collect()
}

/// Points the Workflow's Active Version at a Published Version. Rolling back is pointing it
/// at an older one.
pub async fn set_active_version(
    conn: &mut PgConnection,
    id: &WorkflowId,
    number: VersionNumber,
) -> Result<(), StoreError> {
    let updated = sqlx::query!(
        r#"
        UPDATE workflows SET active_version = v.number
        FROM versions v
        WHERE workflows.id = $1
          AND v.workflow_id = workflows.id
          AND v.number = $2
          AND v.state = 'published'
        "#,
        id.as_str(),
        number.get(),
    )
    .execute(&mut *conn)
    .await?
    .rows_affected();

    if updated == 0 {
        return Err(versions::why_not_published(conn, id, number).await?);
    }
    Ok(())
}
