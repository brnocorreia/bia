use std::future::Future;

use bia_core::dsl::{Definition, WorkflowId};
use chrono::{DateTime, Utc};
use sqlx::types::Json;
use sqlx::{Acquire, PgConnection};

use super::row::VersionRow;
use super::row::version_state;
use super::{Version, VersionNumber};
use crate::StoreError;

/// Versions: immutable, numbered snapshots of a Workflow's Definition (ADR-0009). A Draft
/// can be replaced; publishing freezes it for good.
pub trait VersionRepository {
    /// Saves `definition` as a new Draft of the Workflow it names, numbered one past the
    /// newest Version.
    fn create_draft(
        &self,
        conn: &mut PgConnection,
        definition: &Definition,
    ) -> impl Future<Output = Result<Version, StoreError>> + Send;

    /// Replaces the Definition of a Draft, keeping its number.
    fn replace_draft(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        number: VersionNumber,
        definition: &Definition,
    ) -> impl Future<Output = Result<Version, StoreError>> + Send;

    /// Publishes a Draft at `at`. From then on the Version never changes.
    fn publish(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        number: VersionNumber,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<Version, StoreError>> + Send;

    fn get(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        number: VersionNumber,
    ) -> impl Future<Output = Result<Option<Version>, StoreError>> + Send;

    /// Lists a Workflow's Versions, oldest first.
    fn list(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
    ) -> impl Future<Output = Result<Vec<Version>, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgVersionRepository;

impl VersionRepository for PgVersionRepository {
    async fn create_draft(
        &self,
        conn: &mut PgConnection,
        definition: &Definition,
    ) -> Result<Version, StoreError> {
        let workflow = &definition.id;
        let mut tx = conn.begin().await?;

        let number =
            sqlx::query_file_scalar!("src/version/queries/next_number.sql", workflow.as_str(),)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StoreError::WorkflowNotFound(workflow.clone()))?;

        let row = sqlx::query_file_as!(
            VersionRow,
            "src/version/queries/insert_draft.sql",
            workflow.as_str(),
            number,
            Json(definition) as _,
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;

        row.try_into()
    }

    async fn replace_draft(
        &self,
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

        let row = sqlx::query_file_as!(
            VersionRow,
            "src/version/queries/replace_draft.sql",
            workflow.as_str(),
            number.get(),
            Json(definition) as _,
        )
        .fetch_optional(&mut *conn)
        .await?;

        match row {
            Some(row) => row.try_into(),
            None => Err(why_not_draft(conn, workflow, number).await?),
        }
    }

    async fn publish(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        number: VersionNumber,
        at: DateTime<Utc>,
    ) -> Result<Version, StoreError> {
        let row = sqlx::query_file_as!(
            VersionRow,
            "src/version/queries/publish.sql",
            workflow.as_str(),
            number.get(),
            at,
        )
        .fetch_optional(&mut *conn)
        .await?;

        match row {
            Some(row) => row.try_into(),
            None => Err(why_not_draft(conn, workflow, number).await?),
        }
    }

    async fn get(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
        number: VersionNumber,
    ) -> Result<Option<Version>, StoreError> {
        sqlx::query_file_as!(
            VersionRow,
            "src/version/queries/get.sql",
            workflow.as_str(),
            number.get(),
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(Version::try_from)
        .transpose()
    }

    async fn list(
        &self,
        conn: &mut PgConnection,
        workflow: &WorkflowId,
    ) -> Result<Vec<Version>, StoreError> {
        sqlx::query_file_as!(
            VersionRow,
            "src/version/queries/list.sql",
            workflow.as_str()
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(Version::try_from)
        .collect()
    }
}

/// Explains why a Version could not be used as a Published one.
pub(crate) async fn why_not_published(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
) -> Result<StoreError, StoreError> {
    Ok(if exists(conn, workflow, number).await? {
        // A Version published since the failed statement was still a Draft when it ran.
        StoreError::VersionNotPublished {
            workflow: workflow.clone(),
            number,
        }
    } else {
        missing(conn, workflow, number).await?
    })
}

/// Explains why a Version could not be changed as a Draft.
async fn why_not_draft(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
) -> Result<StoreError, StoreError> {
    Ok(if exists(conn, workflow, number).await? {
        // Published Versions never return to Draft, so an existing one is Published.
        StoreError::VersionPublished {
            workflow: workflow.clone(),
            number,
        }
    } else {
        missing(conn, workflow, number).await?
    })
}

async fn exists(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
) -> Result<bool, StoreError> {
    let state = sqlx::query_file_scalar!(
        "src/version/queries/get_state.sql",
        workflow.as_str(),
        number.get(),
    )
    .fetch_optional(&mut *conn)
    .await?;

    // Reading the state also checks the stored value is one this build knows.
    Ok(state.as_deref().map(version_state).transpose()?.is_some())
}

/// `VersionNotFound`, or `WorkflowNotFound` when the Workflow itself is missing.
async fn missing(
    conn: &mut PgConnection,
    workflow: &WorkflowId,
    number: VersionNumber,
) -> Result<StoreError, StoreError> {
    let workflow_exists =
        sqlx::query_file_scalar!("src/version/queries/workflow_exists.sql", workflow.as_str())
            .fetch_one(&mut *conn)
            .await?;

    Ok(if workflow_exists {
        StoreError::VersionNotFound {
            workflow: workflow.clone(),
            number,
        }
    } else {
        StoreError::WorkflowNotFound(workflow.clone())
    })
}
