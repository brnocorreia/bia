//! Versions: immutable, numbered snapshots of a Workflow's Definition (ADR-0009).
//!
//! A Draft can be replaced; publishing freezes it for good.

use std::fmt;

use bia_core::dsl::{Definition, WorkflowId};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::types::Json;
use sqlx::{Acquire, PgConnection};

use crate::StoreError;

/// A Version's number within its Workflow: 1, 2, 3…
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VersionNumber(i32);

impl VersionNumber {
    /// Returns the number if it is positive.
    pub fn new(number: i32) -> Option<Self> {
        (number > 0).then_some(Self(number))
    }

    pub fn get(self) -> i32 {
        self.0
    }

    pub(crate) fn from_db(number: i32) -> Result<Self, StoreError> {
        Self::new(number).ok_or_else(|| StoreError::invalid("version number", number))
    }
}

impl fmt::Display for VersionNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Whether a Version can still change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VersionState {
    Draft,
    Published,
}

impl VersionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
        }
    }

    fn from_db(state: &str) -> Result<Self, StoreError> {
        match state {
            "draft" => Ok(Self::Draft),
            "published" => Ok(Self::Published),
            other => Err(StoreError::invalid("version state", other)),
        }
    }
}

/// One snapshot of a Workflow's Definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub workflow_id: WorkflowId,
    pub number: VersionNumber,
    pub definition: Definition,
    pub state: VersionState,
    /// When the Version was published; `None` for a Draft.
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// A `versions` row as read back from Postgres.
struct VersionRow {
    workflow_id: String,
    number: i32,
    definition: Value,
    state: String,
    published_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl TryFrom<VersionRow> for Version {
    type Error = StoreError;

    fn try_from(row: VersionRow) -> Result<Self, StoreError> {
        Ok(Self {
            workflow_id: WorkflowId::new(row.workflow_id)
                .map_err(|err| StoreError::invalid("workflow id", err))?,
            number: VersionNumber::from_db(row.number)?,
            definition: Definition::from_value(row.definition)
                .map_err(|err| StoreError::invalid("definition", err))?,
            state: VersionState::from_db(&row.state)?,
            published_at: row.published_at,
            created_at: row.created_at,
        })
    }
}

/// Saves `definition` as a new Draft of the Workflow it names, numbered one past the newest
/// Version.
pub async fn create_draft(
    conn: &mut PgConnection,
    definition: &Definition,
) -> Result<Version, StoreError> {
    let workflow = &definition.id;
    let mut tx = conn.begin().await?;

    // The row lock on the Workflow serialises concurrent saves, so numbers have no gaps.
    let number = sqlx::query_scalar!(
        "UPDATE workflows SET last_version = last_version + 1 WHERE id = $1 RETURNING last_version",
        workflow.as_str(),
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| StoreError::WorkflowNotFound(workflow.clone()))?;

    let row = sqlx::query_as!(
        VersionRow,
        r#"
        INSERT INTO versions (workflow_id, number, definition, state)
        VALUES ($1, $2, $3, 'draft')
        RETURNING workflow_id, number, definition, state, published_at, created_at
        "#,
        workflow.as_str(),
        number,
        Json(definition) as _,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    Version::try_from(row)
}

/// Replaces the Definition of a Draft, keeping its number.
pub async fn replace_draft(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
    definition: &Definition,
) -> Result<Version, StoreError> {
    if definition.id != *workflow {
        return Err(StoreError::DefinitionMismatch {
            workflow: workflow.clone(),
            found: definition.id.clone(),
        });
    }

    let row = sqlx::query_as!(
        VersionRow,
        r#"
        UPDATE versions SET definition = $3
        WHERE workflow_id = $1 AND number = $2 AND state = 'draft'
        RETURNING workflow_id, number, definition, state, published_at, created_at
        "#,
        workflow.as_str(),
        number.get(),
        Json(definition) as _,
    )
    .fetch_optional(&mut *conn)
    .await?;

    match row {
        Some(row) => Version::try_from(row),
        None => Err(why_not_draft(conn, workflow, number).await?),
    }
}

/// Publishes a Draft at `at`. From then on the Version never changes.
pub async fn publish(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
    at: DateTime<Utc>,
) -> Result<Version, StoreError> {
    let row = sqlx::query_as!(
        VersionRow,
        r#"
        UPDATE versions SET state = 'published', published_at = $3
        WHERE workflow_id = $1 AND number = $2 AND state = 'draft'
        RETURNING workflow_id, number, definition, state, published_at, created_at
        "#,
        workflow.as_str(),
        number.get(),
        at,
    )
    .fetch_optional(&mut *conn)
    .await?;

    match row {
        Some(row) => Version::try_from(row),
        None => Err(why_not_draft(conn, workflow, number).await?),
    }
}

/// Reads one Version.
pub async fn get(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
) -> Result<Option<Version>, StoreError> {
    let row = sqlx::query_as!(
        VersionRow,
        r#"
        SELECT workflow_id, number, definition, state, published_at, created_at
        FROM versions
        WHERE workflow_id = $1 AND number = $2
        "#,
        workflow.as_str(),
        number.get(),
    )
    .fetch_optional(&mut *conn)
    .await?;

    row.map(Version::try_from).transpose()
}

/// Lists a Workflow's Versions, oldest first.
pub async fn list(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
) -> Result<Vec<Version>, StoreError> {
    let rows = sqlx::query_as!(
        VersionRow,
        r#"
        SELECT workflow_id, number, definition, state, published_at, created_at
        FROM versions
        WHERE workflow_id = $1
        ORDER BY number
        "#,
        workflow.as_str(),
    )
    .fetch_all(&mut *conn)
    .await?;

    rows.into_iter().map(Version::try_from).collect()
}

/// The state of a Version, or `None` if it does not exist.
async fn state(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
) -> Result<Option<VersionState>, StoreError> {
    let state = sqlx::query_scalar!(
        "SELECT state FROM versions WHERE workflow_id = $1 AND number = $2",
        workflow.as_str(),
        number.get(),
    )
    .fetch_optional(&mut *conn)
    .await?;

    state.as_deref().map(VersionState::from_db).transpose()
}

/// Explains why a Version could not be used as a Published one.
pub(crate) async fn why_not_published(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
) -> Result<StoreError, StoreError> {
    Ok(match state(conn, workflow, number).await? {
        // A Version published since the failed statement was still a Draft when it ran.
        Some(_) => StoreError::VersionNotPublished {
            workflow: workflow.clone(),
            number,
        },
        None => missing(conn, workflow, number).await?,
    })
}

/// Explains why a Version could not be changed as a Draft.
async fn why_not_draft(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
) -> Result<StoreError, StoreError> {
    Ok(match state(conn, workflow, number).await? {
        // Published Versions never return to Draft, so an existing one is Published.
        Some(_) => StoreError::VersionPublished {
            workflow: workflow.clone(),
            number,
        },
        None => missing(conn, workflow, number).await?,
    })
}

/// `VersionNotFound`, or `WorkflowNotFound` when the Workflow itself is missing.
async fn missing(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
) -> Result<StoreError, StoreError> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM workflows WHERE id = $1) AS "exists!""#,
        workflow.as_str(),
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok(if exists {
        StoreError::VersionNotFound {
            workflow: workflow.clone(),
            number,
        }
    } else {
        StoreError::WorkflowNotFound(workflow.clone())
    })
}
