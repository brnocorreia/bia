use std::future::Future;

use bia_core::dsl::WorkflowId;
use sqlx::PgConnection;

use super::Workflow;
use super::row::WorkflowRow;
use crate::StoreError;
use crate::version::VersionNumber;
use crate::version::why_not_published;

/// Workflows: stable identities that own Versions and point at one Active Version.
pub trait WorkflowRepository {
    /// Creates a Workflow with no Versions.
    fn create(
        &self,
        conn: &mut PgConnection,
        id: &WorkflowId,
    ) -> impl Future<Output = Result<Workflow, StoreError>> + Send;

    /// Reads one Workflow.
    fn get(
        &self,
        conn: &mut PgConnection,
        id: &WorkflowId,
    ) -> impl Future<Output = Result<Option<Workflow>, StoreError>> + Send;

    /// Lists every Workflow by id.
    fn list(
        &self,
        conn: &mut PgConnection,
    ) -> impl Future<Output = Result<Vec<Workflow>, StoreError>> + Send;

    /// Points the Active Version at a Published Version. Rolling back is pointing it at an
    /// older one.
    fn set_active_version(
        &self,
        conn: &mut PgConnection,
        id: &WorkflowId,
        number: VersionNumber,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// [`WorkflowRepository`] over Postgres.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgWorkflowRepository;

impl WorkflowRepository for PgWorkflowRepository {
    async fn create(
        &self,
        conn: &mut PgConnection,
        id: &WorkflowId,
    ) -> Result<Workflow, StoreError> {
        sqlx::query_file_as!(WorkflowRow, "src/workflow/queries/insert.sql", id.as_str())
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| StoreError::WorkflowExists(id.clone()))?
            .try_into()
    }

    async fn get(
        &self,
        conn: &mut PgConnection,
        id: &WorkflowId,
    ) -> Result<Option<Workflow>, StoreError> {
        sqlx::query_file_as!(WorkflowRow, "src/workflow/queries/get.sql", id.as_str())
            .fetch_optional(&mut *conn)
            .await?
            .map(Workflow::try_from)
            .transpose()
    }

    async fn list(&self, conn: &mut PgConnection) -> Result<Vec<Workflow>, StoreError> {
        sqlx::query_file_as!(WorkflowRow, "src/workflow/queries/list.sql")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(Workflow::try_from)
            .collect()
    }

    async fn set_active_version(
        &self,
        conn: &mut PgConnection,
        id: &WorkflowId,
        number: VersionNumber,
    ) -> Result<(), StoreError> {
        let updated = sqlx::query_file!(
            "src/workflow/queries/set_active_version.sql",
            id.as_str(),
            number.get(),
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();

        if updated == 0 {
            return Err(why_not_published(conn, id, number).await?);
        }
        Ok(())
    }
}
