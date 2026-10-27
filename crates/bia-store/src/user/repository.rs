use std::future::Future;

use chrono::{DateTime, Utc};
use sqlx::{Connection, PgConnection};

use super::row::UserRow;
use super::{PasswordHash, User};
use crate::StoreError;

/// The advisory lock key, `bia_user` in ASCII.
const SETUP_KEY: i64 = 0x6269_615f_7573_6572;

pub trait UserRepository {
    /// Whether no User can sign in yet: none exists, or its password was reset.
    fn needs_setup(
        &self,
        conn: &mut PgConnection,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Gives the User whose password was reset this password, or creates the first User with
    /// it, and returns `None` once a User has a password. Concurrent callers serialise, so at
    /// most one of them sets it.
    fn set_up(
        &self,
        conn: &mut PgConnection,
        password: &PasswordHash,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<User>, StoreError>> + Send;

    /// Clears every User's password and revokes all their Tokens at `now`, returning how many
    /// Tokens it revoked, or `None` when no User exists.
    fn reset(
        &self,
        conn: &mut PgConnection,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<u64>, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgUserRepository;

impl UserRepository for PgUserRepository {
    async fn needs_setup(&self, conn: &mut PgConnection) -> Result<bool, StoreError> {
        Ok(sqlx::query_file_scalar!("src/user/queries/needs_setup.sql")
            .fetch_one(&mut *conn)
            .await?)
    }

    async fn set_up(
        &self,
        conn: &mut PgConnection,
        password: &PasswordHash,
        now: DateTime<Utc>,
    ) -> Result<Option<User>, StoreError> {
        let mut tx = conn.begin().await?;
        sqlx::query_file!("src/user/queries/lock.sql", SETUP_KEY)
            .execute(&mut *tx)
            .await?;
        let mut row = sqlx::query_file_as!(
            UserRow,
            "src/user/queries/set_missing_password.sql",
            password.0
        )
        .fetch_optional(&mut *tx)
        .await?;
        if row.is_none() {
            row = sqlx::query_file_as!(
                UserRow,
                "src/user/queries/insert_first.sql",
                password.0,
                now
            )
            .fetch_optional(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(row.map(User::from))
    }

    async fn reset(
        &self,
        conn: &mut PgConnection,
        now: DateTime<Utc>,
    ) -> Result<Option<u64>, StoreError> {
        let mut tx = conn.begin().await?;
        let users = sqlx::query_file_scalar!("src/user/queries/clear_passwords.sql")
            .fetch_all(&mut *tx)
            .await?;
        if users.is_empty() {
            return Ok(None);
        }
        let mut revoked = 0;
        for user in users {
            revoked += sqlx::query_file!("src/user/queries/revoke_for_user.sql", user, now)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        }
        tx.commit().await?;
        Ok(Some(revoked))
    }
}
