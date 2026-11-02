use std::future::Future;

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::PgConnection;

use super::row::TokenRow;
use super::{NewToken, Scope, Token, TokenHash, TokenId};
use crate::StoreError;
use crate::user::UserId;

/// How stale `last_used_at` may get before a use records it again.
const LAST_USED_RESOLUTION: TimeDelta = TimeDelta::minutes(1);

pub trait TokenRepository {
    fn create(
        &self,
        conn: &mut PgConnection,
        token: &NewToken,
    ) -> impl Future<Output = Result<Token, StoreError>> + Send;

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

    /// Revokes every Session of the User but `keep`, returning how many it revoked.
    fn revoke_sessions_except(
        &self,
        conn: &mut PgConnection,
        user: UserId,
        keep: Option<TokenId>,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// The unrevoked, unexpired Token with this hash, recording that it was used at `now`.
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
            token.user_id.as_uuid(),
            token.created_at,
            token.origin.as_str(),
        )
        .fetch_one(&mut *conn)
        .await?
        .try_into()
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

    async fn revoke_sessions_except(
        &self,
        conn: &mut PgConnection,
        user: UserId,
        keep: Option<TokenId>,
        now: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        Ok(sqlx::query_file!(
            "src/token/queries/revoke_sessions.sql",
            user.as_uuid(),
            keep.map(TokenId::as_uuid),
            now
        )
        .execute(&mut *conn)
        .await?
        .rows_affected())
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
        if token.is_expired(now) {
            return Ok(None);
        }
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
