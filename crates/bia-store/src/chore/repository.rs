use std::future::Future;

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::PgConnection;

use super::ChoreClaim;
use crate::StoreError;

pub trait ChoreRepository {
    /// Claims the run of `chore` at `now` when it has not run within `interval` before it.
    /// Among callers racing for the same interval, exactly one gets
    /// [`Claimed`](ChoreClaim::Claimed). Call it outside a transaction: the claim must commit
    /// before the Chore's work, so a Chore that fails waits for its next interval.
    fn claim(
        &self,
        conn: &mut PgConnection,
        chore: &str,
        now: DateTime<Utc>,
        interval: TimeDelta,
    ) -> impl Future<Output = Result<ChoreClaim, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgChoreRepository;

impl ChoreRepository for PgChoreRepository {
    async fn claim(
        &self,
        conn: &mut PgConnection,
        chore: &str,
        now: DateTime<Utc>,
        interval: TimeDelta,
    ) -> Result<ChoreClaim, StoreError> {
        let claimed =
            sqlx::query_file_scalar!("src/chore/queries/claim.sql", chore, now, now - interval)
                .fetch_optional(&mut *conn)
                .await?
                .is_some();
        if claimed {
            return Ok(ChoreClaim::Claimed);
        }
        let ran_at = sqlx::query_file_scalar!("src/chore/queries/ran_at.sql", chore)
            .fetch_one(&mut *conn)
            .await?;
        Ok(ChoreClaim::RanAt(ran_at))
    }
}
