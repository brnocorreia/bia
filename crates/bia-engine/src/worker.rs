//! The Worker loop: claims Jobs and advances their Runs, up to a concurrency limit.
//!
//! A Worker claims whenever it has a free slot: on start, when a Job finishes, when Postgres
//! notifies that a Job became claimable, and on a poll timer, because a delayed Job falling
//! due or an expired Lease notifies nothing (ADR-0031). Each claimed Job runs in its own task
//! next to a heartbeat that extends its Lease every third of the Lease duration and re-reads
//! the Run's cancel flag, which covers cancel notifications lost while the listener
//! reconnected. On shutdown the Worker stops claiming, gives in-flight Jobs a grace period,
//! then stops the rest and releases their Leases so another Worker takes them over at once
//! (ADR-0036).

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;

use bia_core::action::CancellationToken;
use bia_store::StoreError;
use bia_store::job::{
    Backoff, Claimed, JobListener, JobRepository, Lease, PgJobRepository, Wakeup,
};
use bia_store::run::{PgRunRepository, RunId, RunRepository};
use chrono::TimeDelta;
use tokio::task::{self, JoinSet};
use tracing::Instrument;

use crate::{EngineError, Interpreter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConfig {
    /// How many Jobs the Worker runs at once.
    pub concurrency: NonZeroUsize,
    /// How long a claim holds a Job before another Worker may take it. The heartbeat extends
    /// it while the Job runs, so it bounds how long a crashed Worker's Jobs stay stuck.
    pub lease: TimeDelta,
    /// How often the Worker claims when no notification woke it.
    pub poll_interval: TimeDelta,
    /// How long in-flight Jobs may keep running after shutdown starts.
    pub shutdown_grace: TimeDelta,
    /// The delay before an `advance_run` Job is retried after the engine failed it.
    pub job_backoff: Backoff,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            concurrency: NonZeroUsize::new(16).unwrap_or(NonZeroUsize::MIN),
            lease: TimeDelta::seconds(30),
            poll_interval: TimeDelta::seconds(5),
            shutdown_grace: TimeDelta::seconds(20),
            job_backoff: Backoff::default(),
        }
    }
}

/// Claims Jobs and runs them through the interpreter. Any number of Workers can share a
/// database: claims skip each other's Jobs and Leases are fenced (ADR-0031).
pub struct Worker {
    shared: Arc<Shared>,
}

struct Shared {
    id: String,
    interpreter: Interpreter,
    config: WorkerConfig,
}

impl fmt::Debug for Worker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Worker")
            .field("id", &self.shared.id)
            .field("config", &self.shared.config)
            .finish_non_exhaustive()
    }
}

/// A Job running in its own task.
struct InFlight {
    lease: Lease,
    run: Option<RunId>,
    cancel: CancellationToken,
}

impl Worker {
    /// `id` names the Worker in the Leases it holds, for diagnosis only: correctness never
    /// depends on it being unique.
    pub fn new(id: impl Into<String>, interpreter: Interpreter, config: WorkerConfig) -> Self {
        Self {
            shared: Arc::new(Shared {
                id: id.into(),
                interpreter,
                config,
            }),
        }
    }

    /// Runs until `shutdown` fires and the shutdown sequence ends. Errors only when it cannot
    /// start listening; a database that fails later is logged and retried on the next poll.
    pub async fn run(&self, shutdown: CancellationToken) -> Result<(), EngineError> {
        let shared = &self.shared;
        let span = tracing::info_span!("worker", worker = %shared.id);
        async {
            let mut listener = JobListener::connect(shared.interpreter.pool()).await?;
            tracing::info!(
                concurrency = shared.config.concurrency.get(),
                "worker started"
            );
            let mut tasks = JoinSet::new();
            let mut in_flight = HashMap::new();
            // Cleared when the listener fails, so a lost connection does not spin the loop;
            // the next poll tries it again.
            let mut listening = true;

            loop {
                let free = shared.config.concurrency.get() - in_flight.len();
                if free > 0 {
                    for claimed in shared.claim(free).await {
                        let cancel = CancellationToken::new();
                        let entry = InFlight {
                            lease: claimed.lease.clone(),
                            run: claimed.job.run_id,
                            cancel: cancel.clone(),
                        };
                        // Created here, so the job's span sits inside the Worker's.
                        let span = tracing::info_span!(
                            "job",
                            job_id = %claimed.job.id,
                            // Not `attempt`, which names a Step's Attempt in the same lines.
                            claim = claimed.lease.attempt(),
                        );
                        let work = Arc::clone(shared).work(claimed, cancel).instrument(span);
                        let handle = tasks.spawn(work);
                        in_flight.insert(handle.id(), entry);
                    }
                }

                let poll = shared.clock().now() + shared.config.poll_interval;
                tokio::select! {
                    // Shutdown first, so a notification arriving with it claims nothing.
                    biased;
                    () = shutdown.cancelled() => break,
                    Some(done) = tasks.join_next_with_id() => {
                        shared.finished(done, &mut in_flight).await;
                    }
                    wakeup = listener.recv(), if listening => match wakeup {
                        Ok(Wakeup::CancelRequested(run)) => cancel_attempts(&in_flight, run),
                        Ok(Wakeup::JobClaimable(_)) => {}
                        Err(error) => {
                            tracing::warn!(%error, "job listener failed; polling until it is back");
                            listening = false;
                        }
                    },
                    () = shared.clock().sleep_until(poll) => listening = true,
                }
            }

            shared.shut_down(tasks, in_flight).await;
            Ok(())
        }
        .instrument(span)
        .await
    }
}

fn cancel_attempts(in_flight: &HashMap<task::Id, InFlight>, run: RunId) {
    for job in in_flight.values().filter(|job| job.run == Some(run)) {
        tracing::info!(run_id = %run, "cancelling in-flight attempt");
        job.cancel.cancel();
    }
}

impl Shared {
    fn clock(&self) -> &dyn bia_core::clock::Clock {
        self.interpreter.clock()
    }

    async fn claim(&self, limit: usize) -> Vec<Claimed> {
        let claimed = async {
            let mut conn = self.interpreter.pool().acquire().await?;
            PgJobRepository
                .claim(
                    &mut conn,
                    &self.id,
                    i64::try_from(limit).unwrap_or(i64::MAX),
                    self.config.lease,
                    self.clock().now(),
                )
                .await
        };
        claimed.await.unwrap_or_else(|error: StoreError| {
            tracing::error!(%error, "cannot claim jobs");
            Vec::new()
        })
    }

    async fn work(self: Arc<Self>, claimed: Claimed, cancel: CancellationToken) {
        let result = tokio::select! {
            result = self.interpreter.advance_cancellable(&claimed, &cancel) => result,
            lost = self.heartbeat(&claimed, &cancel) => Err(lost.into()),
        };
        match result {
            Ok(_) => {}
            Err(EngineError::Store(StoreError::LeaseLost(_))) => {
                tracing::warn!("lease lost to another worker; the job's outcome is theirs");
            }
            Err(error) => self.fail(&claimed.lease, &error.to_string()).await,
        }
    }

    /// Extends the Lease and re-reads the Run's cancel flag every third of the Lease
    /// duration. Returns only once the Lease is lost, which ends the Attempt: another Worker
    /// owns the Job now.
    async fn heartbeat(&self, claimed: &Claimed, cancel: &CancellationToken) -> StoreError {
        let every = self.config.lease / 3;
        let mut lease = claimed.lease.clone();
        // Beats are timed from the Lease, not from when the last one ended, so a slow beat
        // does not push the next one towards the Lease's expiry.
        let mut next = lease.until() - (self.config.lease - every);
        loop {
            self.clock().sleep_until(next).await;
            let beat = async {
                let mut conn = self.interpreter.pool().acquire().await?;
                lease = PgJobRepository
                    .extend_lease(&mut conn, &lease, self.config.lease, self.clock().now())
                    .await?;
                if let Some(run) = claimed.job.run_id {
                    let flagged = PgRunRepository
                        .get(&mut conn, run)
                        .await?
                        .is_some_and(|run| run.cancel_requested_at.is_some());
                    if flagged && !cancel.is_cancelled() {
                        tracing::info!("run cancelled; cancelling the in-flight attempt");
                        cancel.cancel();
                    }
                }
                Ok::<_, StoreError>(())
            };
            match beat.await {
                Ok(()) => next = lease.until() - (self.config.lease - every),
                Err(lost @ StoreError::LeaseLost(_)) => return lost,
                Err(error) => {
                    tracing::warn!(%error, "cannot extend lease; retrying");
                    next = self.clock().now() + every;
                }
            }
        }
    }

    async fn fail(&self, lease: &Lease, error: &str) {
        tracing::error!(
            error,
            "engine failed the job; it is retried after a backoff"
        );
        let failed = async {
            let mut conn = self.interpreter.pool().acquire().await?;
            PgJobRepository
                .fail(
                    &mut conn,
                    lease,
                    error,
                    &self.config.job_backoff,
                    self.clock().now(),
                )
                .await
        };
        if let Err(error) = failed.await {
            tracing::error!(%error, "cannot record the job failure; its lease will expire");
        }
    }

    async fn finished(
        &self,
        done: Result<(task::Id, ()), task::JoinError>,
        in_flight: &mut HashMap<task::Id, InFlight>,
    ) {
        let id = match done {
            Ok((id, ())) => id,
            Err(error) => {
                let id = error.id();
                if let Some(job) = in_flight.get(&id) {
                    tracing::error!(job_id = %job.lease.job_id(), %error, "job task panicked");
                    self.fail(&job.lease.clone(), "the job's task panicked")
                        .await;
                }
                id
            }
        };
        in_flight.remove(&id);
    }

    async fn shut_down(&self, mut tasks: JoinSet<()>, mut in_flight: HashMap<task::Id, InFlight>) {
        tracing::info!(
            in_flight = in_flight.len(),
            "shutting down: no longer claiming, waiting for in-flight jobs"
        );
        let grace = self
            .clock()
            .sleep_until(self.clock().now() + self.config.shutdown_grace);
        tokio::pin!(grace);
        while !in_flight.is_empty() {
            tokio::select! {
                Some(done) = tasks.join_next_with_id() => self.finished(done, &mut in_flight).await,
                () = &mut grace => break,
            }
        }

        // Dropping a task's future rolls back any transaction it had open, so once it is
        // gone the Lease can be released without the Job also completing.
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        for job in in_flight.into_values() {
            let released = async {
                let mut conn = self.interpreter.pool().acquire().await?;
                PgJobRepository.release(&mut conn, &job.lease).await
            };
            match released.await {
                Ok(()) => tracing::info!(job_id = %job.lease.job_id(), "released unfinished job"),
                // It committed just before it was stopped.
                Err(StoreError::LeaseLost(_)) => {}
                Err(error) => tracing::warn!(
                    job_id = %job.lease.job_id(),
                    %error,
                    "cannot release lease; the job is claimable once it expires"
                ),
            }
        }
        tracing::info!("worker stopped");
    }
}
