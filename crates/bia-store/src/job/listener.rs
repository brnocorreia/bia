use sqlx::PgPool;
use sqlx::postgres::PgListener;
use uuid::Uuid;

use super::JobKind;
use super::row::job_kind;
use crate::StoreError;
use crate::run::RunId;

/// The channel an enqueue or a released Lease notifies once its transaction commits; the
/// payload is the Job kind. The `jobs_enqueued` trigger sends it, so every enqueue path does.
pub const NOTIFY_CHANNEL: &str = "bia_jobs";

/// The channel a Run's cancel request notifies once its transaction commits; the payload is
/// the Run id. The `runs_cancel_requested` trigger sends it.
pub const CANCEL_CHANNEL: &str = "bia_run_cancels";

/// What a Worker hears from Postgres.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wakeup {
    /// A Job of this kind became claimable.
    JobClaimable(JobKind),
    /// The Run was flagged for cancellation.
    CancelRequested(RunId),
}

/// Wakes an idle claimer when a Job becomes claimable, so it does not have to poll, and tells
/// Workers about cancel requests.
///
/// Only enqueues and released Leases notify: a delayed Job falling due, an expired Lease and a
/// retry do not, and notifications sent while the connection is re-established are lost. A
/// claimer therefore also claims on a timer, and treats a notification as a hint to claim now.
#[derive(Debug)]
pub struct JobListener {
    listener: PgListener,
}

impl JobListener {
    pub async fn connect(pool: &PgPool) -> Result<Self, StoreError> {
        let mut listener = PgListener::connect_with(pool).await?;
        listener
            .listen_all([NOTIFY_CHANNEL, CANCEL_CHANNEL])
            .await?;
        Ok(Self { listener })
    }

    pub async fn recv(&mut self) -> Result<Wakeup, StoreError> {
        let notification = self.listener.recv().await?;
        let payload = notification.payload();
        if notification.channel() == CANCEL_CHANNEL {
            let run = Uuid::parse_str(payload).map_err(|err| StoreError::invalid("run id", err))?;
            return Ok(Wakeup::CancelRequested(RunId::from_uuid(run)));
        }
        Ok(Wakeup::JobClaimable(job_kind(payload)?))
    }
}
