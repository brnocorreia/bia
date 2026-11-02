use std::future::Future;

use chrono::{DateTime, Utc};
use sqlx::{Connection, PgConnection};

use super::row::PasskeyRow;
use super::{CeremonyId, CeremonyKind, NewPasskey, Passkey, PasskeyId};
use crate::StoreError;
use crate::user::UserId;

pub trait PasskeyRepository {
    /// The User's passkeys, oldest first.
    fn list(
        &self,
        conn: &mut PgConnection,
        user: UserId,
    ) -> impl Future<Output = Result<Vec<Passkey>, StoreError>> + Send;

    fn create(
        &self,
        conn: &mut PgConnection,
        passkey: &NewPasskey,
    ) -> impl Future<Output = Result<Passkey, StoreError>> + Send;

    /// `None` when the User has no passkey with this id.
    fn rename(
        &self,
        conn: &mut PgConnection,
        user: UserId,
        id: PasskeyId,
        name: &str,
    ) -> impl Future<Output = Result<Option<Passkey>, StoreError>> + Send;

    /// Returns whether the User had a passkey with this id.
    fn delete(
        &self,
        conn: &mut PgConnection,
        user: UserId,
        id: PasskeyId,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Records a sign-in with the passkey at `now`, and the credential as the sign-in left it
    /// (its signature counter moves).
    fn record_use(
        &self,
        conn: &mut PgConnection,
        id: PasskeyId,
        credential: &serde_json::Value,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Keeps the state of a ceremony until `expires_at`, and deletes ceremonies that already
    /// expired at `now`.
    fn begin_ceremony(
        &self,
        conn: &mut PgConnection,
        user: UserId,
        kind: CeremonyKind,
        state: &serde_json::Value,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<CeremonyId, StoreError>> + Send;

    /// The state of the User's ceremony of this kind, which cannot be taken again; `None` when
    /// there is none, it belongs to someone else, or it expired at `now`.
    fn take_ceremony(
        &self,
        conn: &mut PgConnection,
        user: UserId,
        id: CeremonyId,
        kind: CeremonyKind,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<serde_json::Value>, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgPasskeyRepository;

impl PasskeyRepository for PgPasskeyRepository {
    async fn list(
        &self,
        conn: &mut PgConnection,
        user: UserId,
    ) -> Result<Vec<Passkey>, StoreError> {
        Ok(
            sqlx::query_file_as!(PasskeyRow, "src/passkey/queries/list.sql", user.as_uuid())
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(Passkey::from)
                .collect(),
        )
    }

    async fn create(
        &self,
        conn: &mut PgConnection,
        passkey: &NewPasskey,
    ) -> Result<Passkey, StoreError> {
        Ok(sqlx::query_file_as!(
            PasskeyRow,
            "src/passkey/queries/insert.sql",
            passkey.user_id.as_uuid(),
            passkey.name,
            passkey.credential,
            passkey.created_at
        )
        .fetch_one(&mut *conn)
        .await?
        .into())
    }

    async fn rename(
        &self,
        conn: &mut PgConnection,
        user: UserId,
        id: PasskeyId,
        name: &str,
    ) -> Result<Option<Passkey>, StoreError> {
        Ok(sqlx::query_file_as!(
            PasskeyRow,
            "src/passkey/queries/rename.sql",
            id.as_uuid(),
            user.as_uuid(),
            name
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(Passkey::from))
    }

    async fn delete(
        &self,
        conn: &mut PgConnection,
        user: UserId,
        id: PasskeyId,
    ) -> Result<bool, StoreError> {
        Ok(sqlx::query_file!(
            "src/passkey/queries/delete.sql",
            id.as_uuid(),
            user.as_uuid()
        )
        .execute(&mut *conn)
        .await?
        .rows_affected()
            == 1)
    }

    async fn record_use(
        &self,
        conn: &mut PgConnection,
        id: PasskeyId,
        credential: &serde_json::Value,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        sqlx::query_file!(
            "src/passkey/queries/record_use.sql",
            id.as_uuid(),
            credential,
            now
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    async fn begin_ceremony(
        &self,
        conn: &mut PgConnection,
        user: UserId,
        kind: CeremonyKind,
        state: &serde_json::Value,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<CeremonyId, StoreError> {
        let mut tx = conn.begin().await?;
        sqlx::query_file!("src/passkey/queries/purge_ceremonies.sql", now)
            .execute(&mut *tx)
            .await?;
        let id = sqlx::query_file_scalar!(
            "src/passkey/queries/insert_ceremony.sql",
            user.as_uuid(),
            kind.as_str(),
            state,
            expires_at
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(CeremonyId::from_uuid(id))
    }

    async fn take_ceremony(
        &self,
        conn: &mut PgConnection,
        user: UserId,
        id: CeremonyId,
        kind: CeremonyKind,
        now: DateTime<Utc>,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        Ok(sqlx::query_file_scalar!(
            "src/passkey/queries/take_ceremony.sql",
            id.as_uuid(),
            user.as_uuid(),
            kind.as_str(),
            now
        )
        .fetch_optional(&mut *conn)
        .await?)
    }
}
