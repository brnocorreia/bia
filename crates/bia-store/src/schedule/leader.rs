use std::fmt;

use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, PgConnection, PgPool};

use crate::StoreError;

/// The advisory lock key, `bia_schd` in ASCII.
const KEY: i64 = 0x6269_615f_7363_6864;

/// The session-level advisory lock that makes one scheduler the leader.
///
/// The lock lives on a connection of its own, outside the pool, because a session lock stays
/// with its connection: a pooled one would carry it back into the pool. When the process dies
/// or the connection drops, Postgres releases the lock and another scheduler can take it.
/// Leadership only saves work: firing stays exactly-once without it (ADR-0039).
pub struct LeaderLock {
    options: PgConnectOptions,
    conn: Option<PgConnection>,
    held: bool,
}

impl fmt::Debug for LeaderLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaderLock")
            .field("held", &self.held)
            .finish_non_exhaustive()
    }
}

impl LeaderLock {
    /// Connects lazily, on the first [`try_hold`](Self::try_hold).
    pub fn new(pool: &PgPool) -> Self {
        Self {
            options: (*pool.connect_options()).clone(),
            conn: None,
            held: false,
        }
    }

    /// Takes the lock if it is free, or checks that this holder still has it. Returns whether
    /// this holder leads. An error means the connection failed, which also lost the lock; the
    /// next call reconnects.
    pub async fn try_hold(&mut self) -> Result<bool, StoreError> {
        let result = self.hold().await;
        if result.is_err() {
            self.conn = None;
            self.held = false;
        }
        result
    }

    async fn hold(&mut self) -> Result<bool, StoreError> {
        let conn = match &mut self.conn {
            Some(conn) => conn,
            None => self
                .conn
                .insert(PgConnection::connect_with(&self.options).await?),
        };
        if self.held {
            // A live session still holds its lock; taking it again would only stack it.
            sqlx::query_file_scalar!("src/schedule/queries/still_connected.sql")
                .fetch_one(&mut *conn)
                .await?;
        } else {
            self.held = sqlx::query_file_scalar!("src/schedule/queries/try_lead.sql", KEY)
                .fetch_one(&mut *conn)
                .await?;
        }
        Ok(self.held)
    }

    /// Gives up the lock, so another scheduler can lead at once.
    pub async fn release(&mut self) -> Result<(), StoreError> {
        let held = std::mem::take(&mut self.held);
        let Some(mut conn) = self.conn.take() else {
            return Ok(());
        };
        if held {
            sqlx::query_file_scalar!("src/schedule/queries/unlock.sql", KEY)
                .fetch_one(&mut conn)
                .await?;
        }
        conn.close().await?;
        Ok(())
    }
}
