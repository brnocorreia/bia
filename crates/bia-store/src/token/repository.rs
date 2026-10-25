use std::future::Future;

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::{Connection, PgConnection};

use super::row::TokenRow;
use super::{NewToken, Scope, Token, TokenHash, TokenId};
use crate::StoreError;

/// The advisory lock key, `bia_tokn` in ASCII.
const ADMINS_KEY: i64 = 0x6269_615f_746f_6b6e;

/// How stale `last_used_at` may get before a use records it again.
const LAST_USED_RESOLUTION: TimeDelta = TimeDelta::minutes(1);

pub trait TokenRepository {
    fn create(
        &self,
        conn: &mut PgConnection,
        token: &NewToken,
    ) -> impl Future<Output = Result<Token, StoreError>> + Send;

    /// Creates an admin Token named `name` only while no unrevoked admin Token exists, and
    /// returns `None` otherwise. Concurrent callers serialise, so at most one of them gets a
    /// Token.
    fn create_first_admin(
        &self,
        conn: &mut PgConnection,
        name: &str,
        hash: TokenHash,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<Token>, StoreError>> + Send;

    /// Every Token, revoked ones included, oldest first.
    fn list(
        &self,
        conn: &mut PgConnection,
    ) -> impl Future<Output = Result<Vec<Token>, StoreError>> + Send;

    /// Revokes the Token, keeping the first revocation time if it already was; `None` when no
    /// Token has this id.
    fn revoke(
        &self,
        conn: &mut PgConnection,
        id: TokenId,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<Token>, StoreError>> + Send;

    /// The unrevoked Token with this hash, recording that it was used at `now`.
    fn authenticate(
        &self,
        conn: &mut PgConnection,
        hash: TokenHash,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<Token>, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgTokenRepository;

impl TokenRepository for PgTokenRepository {
    async fn create(&self, conn: &mut PgConnection, token: &NewToken) -> Result<Token, StoreError> {
        let (workflow_id, trigger_id) = match &token.scope {
            Scope::Webhook {
                workflow_id,
                trigger_id,
            } => (Some(workflow_id.as_str()), Some(trigger_id.as_str())),
            Scope::Admin | Scope::Agent => (None, None),
        };
        sqlx::query_file_as!(
            TokenRow,
            "src/token/queries/insert.sql",
            token.name,
            &token.hash.0[..],
            token.scope.as_str(),
            workflow_id,
            trigger_id,
            token.created_at,
        )
        .fetch_one(&mut *conn)
        .await?
        .try_into()
    }

    async fn create_first_admin(
        &self,
        conn: &mut PgConnection,
        name: &str,
        hash: TokenHash,
        now: DateTime<Utc>,
    ) -> Result<Option<Token>, StoreError> {
        let mut tx = conn.begin().await?;
        sqlx::query_file!("src/token/queries/lock_admins.sql", ADMINS_KEY)
            .execute(&mut *tx)
            .await?;
        let token = sqlx::query_file_as!(
            TokenRow,
            "src/token/queries/insert_first_admin.sql",
            name,
            &hash.0[..],
            now,
        )
        .fetch_optional(&mut *tx)
        .await?
        .map(Token::try_from)
        .transpose()?;
        tx.commit().await?;
        Ok(token)
    }

    async fn list(&self, conn: &mut PgConnection) -> Result<Vec<Token>, StoreError> {
        sqlx::query_file_as!(TokenRow, "src/token/queries/list.sql")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(Token::try_from)
            .collect()
    }

    async fn revoke(
        &self,
        conn: &mut PgConnection,
        id: TokenId,
        now: DateTime<Utc>,
    ) -> Result<Option<Token>, StoreError> {
        sqlx::query_file_as!(TokenRow, "src/token/queries/revoke.sql", id.as_uuid(), now)
            .fetch_optional(&mut *conn)
            .await?
            .map(Token::try_from)
            .transpose()
    }

    async fn authenticate(
        &self,
        conn: &mut PgConnection,
        hash: TokenHash,
        now: DateTime<Utc>,
    ) -> Result<Option<Token>, StoreError> {
        let Some(row) =
            sqlx::query_file_as!(TokenRow, "src/token/queries/get_unrevoked.sql", &hash.0[..])
                .fetch_optional(&mut *conn)
                .await?
        else {
            return Ok(None);
        };
        let mut token = Token::try_from(row)?;
        if token
            .last_used_at
            .is_none_or(|used| used <= now - LAST_USED_RESOLUTION)
        {
            sqlx::query_file!("src/token/queries/touch.sql", token.id.as_uuid(), now)
                .execute(&mut *conn)
                .await?;
            token.last_used_at = Some(now);
        }
        Ok(Some(token))
    }
}
