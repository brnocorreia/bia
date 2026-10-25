use std::future::Future;

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

use super::{Sealed, SecretInfo, SecretName};
use crate::StoreError;

pub trait SecretRepository {
    /// Creates the Secret or replaces its value, keeping `created_at`.
    fn put(
        &self,
        conn: &mut PgConnection,
        name: &SecretName,
        sealed: &Sealed,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<SecretInfo, StoreError>> + Send;

    /// Every Secret by name, without values.
    fn list(
        &self,
        conn: &mut PgConnection,
    ) -> impl Future<Output = Result<Vec<SecretInfo>, StoreError>> + Send;

    /// Whether a Secret with this name existed.
    fn delete(
        &self,
        conn: &mut PgConnection,
        name: &SecretName,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    fn get(
        &self,
        conn: &mut PgConnection,
        name: &SecretName,
    ) -> impl Future<Output = Result<Option<Sealed>, StoreError>> + Send;

    /// Whether any Secret exists.
    fn any(&self, conn: &mut PgConnection)
    -> impl Future<Output = Result<bool, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgSecretRepository;

struct InfoRow {
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<InfoRow> for SecretInfo {
    type Error = StoreError;

    fn try_from(row: InfoRow) -> Result<Self, StoreError> {
        Ok(Self {
            name: SecretName::new(row.name).map_err(|e| StoreError::invalid("secret name", e))?,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

impl SecretRepository for PgSecretRepository {
    async fn put(
        &self,
        conn: &mut PgConnection,
        name: &SecretName,
        sealed: &Sealed,
        now: DateTime<Utc>,
    ) -> Result<SecretInfo, StoreError> {
        sqlx::query_file_as!(
            InfoRow,
            "src/secret/queries/upsert.sql",
            name.as_str(),
            sealed.ciphertext,
            &sealed.nonce[..],
            now,
        )
        .fetch_one(&mut *conn)
        .await?
        .try_into()
    }

    async fn list(&self, conn: &mut PgConnection) -> Result<Vec<SecretInfo>, StoreError> {
        sqlx::query_file_as!(InfoRow, "src/secret/queries/list.sql")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(SecretInfo::try_from)
            .collect()
    }

    async fn delete(&self, conn: &mut PgConnection, name: &SecretName) -> Result<bool, StoreError> {
        let done = sqlx::query_file!("src/secret/queries/delete.sql", name.as_str())
            .execute(&mut *conn)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    async fn get(
        &self,
        conn: &mut PgConnection,
        name: &SecretName,
    ) -> Result<Option<Sealed>, StoreError> {
        let Some(row) = sqlx::query_file!("src/secret/queries/get.sql", name.as_str())
            .fetch_optional(&mut *conn)
            .await?
        else {
            return Ok(None);
        };
        let nonce = row
            .nonce
            .try_into()
            .map_err(|_| StoreError::invalid("secret nonce", "not 24 bytes"))?;
        Ok(Some(Sealed {
            nonce,
            ciphertext: row.ciphertext,
        }))
    }

    async fn any(&self, conn: &mut PgConnection) -> Result<bool, StoreError> {
        Ok(sqlx::query_file!("src/secret/queries/exists.sql")
            .fetch_one(&mut *conn)
            .await?
            .exists)
    }
}
