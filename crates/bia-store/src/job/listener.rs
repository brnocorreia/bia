use sqlx::PgPool;
use sqlx::postgres::PgListener;

use super::JobKind;
use super::row::job_kind;
use crate::StoreError;

/// The channel an enqueue notifies once its transaction commits; the payload is the Job kind.
/// The `jobs_enqueued` trigger sends it, so every enqueue path does.
pub const NOTIFY_CHANNEL: &str = "bia_jobs";

/// Wakes an idle claimer when a Job is enqueued, so it does not have to poll.
///
/// Only enqueues notify: a delayed Job falling due, an expired Lease and a retry do not, and
/// notifications sent while the connection is re-established are lost. A claimer therefore
/// also claims on a timer, and treats a notification as a hint to claim now.
#[derive(Debug)]
pub struct JobListener {
    listener: PgListener,
}

impl JobListener {
    pub async fn connect(pool: &PgPool) -> Result<Self, StoreError> {
        let mut listener = PgListener::connect_with(pool).await?;
        listener.listen(NOTIFY_CHANNEL).await?;
        Ok(Self { listener })
    }

    pub async fn recv(&mut self) -> Result<JobKind, StoreError> {
        let notification = self.listener.recv().await?;
        job_kind(notification.payload())
    }
}
