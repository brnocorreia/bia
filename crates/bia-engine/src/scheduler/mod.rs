//! The scheduler: fires the cron Triggers of Active Versions.
//!
//! Every scheduler replica polls, and the one holding the leader lock works on each poll: it
//! rebuilds the Schedules of Workflows whose Active Version changed, fires each due Schedule in
//! a transaction of its own, and runs the [`Chore`]s that are due. Firing claims the Schedule's
//! row and creates the Run with its first Job in the same transaction, so an occurrence fires
//! exactly once even while two schedulers both believe they lead (ADR-0039). Which occurrences
//! fire is ADR-0040. A Chore's run is claimed the same way, once per interval (ADR-0041).

use std::fmt;
use std::sync::Arc;

use bia_core::action::{BoxFuture, CancellationToken};
use bia_core::clock::Clock;
use bia_core::dsl::{CronTrigger, TriggerKind, WorkflowId};
use bia_store::StoreError;
use bia_store::chore::{ChoreClaim, ChoreRepository, PgChoreRepository};
use bia_store::run::RunId;
use bia_store::schedule::{
    DueSchedule, LeaderLock, PgScheduleRepository, Schedule, ScheduleRepository,
};
use bia_store::version::{PgVersionRepository, VersionNumber, VersionRepository};
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use tracing::Instrument;

use crate::{EngineError, start_run};

mod plan;

pub use plan::{CATCH_UP_LIMIT, MISSED_AFTER, QUEUE_LIMIT};
use plan::{due_at, plan};

/// The most Schedules one poll fires, so a backlog cannot hold off leadership checks.
const FIRES_PER_POLL: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// How often the scheduler checks for due Schedules and for leadership. It bounds how late
    /// an occurrence fires, and how long a new leader takes to notice the old one is gone.
    pub poll_interval: TimeDelta,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            poll_interval: TimeDelta::seconds(1),
        }
    }
}

/// Periodic upkeep the leading scheduler runs, such as Run retention.
///
/// A Chore runs on the leader once every [`interval`](Chore::interval), counted from its last
/// run by any scheduler, which Postgres records under its [`name`](Chore::name) (ADR-0041). The
/// run is claimed before the Chore starts, so one that fails or is cut short waits for its next
/// interval and must leave its work safe to resume then.
pub trait Chore: fmt::Debug + Send + Sync {
    /// Identifies the Chore's last run across schedulers and releases, so it must not change.
    fn name(&self) -> &str;

    fn interval(&self) -> TimeDelta;

    fn run<'a>(
        &'a self,
        pool: &'a PgPool,
        now: DateTime<Utc>,
    ) -> BoxFuture<'a, Result<(), EngineError>>;
}

/// Fires Schedules and runs Chores while it leads. Any number of schedulers can share a
/// database.
pub struct Scheduler {
    id: String,
    pool: PgPool,
    clock: Arc<dyn Clock>,
    config: SchedulerConfig,
    chores: Vec<Arc<dyn Chore>>,
}

impl fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scheduler")
            .field("id", &self.id)
            .field("config", &self.config)
            .field("chores", &self.chores)
            .finish_non_exhaustive()
    }
}

impl Scheduler {
    /// `id` names the scheduler in logs only.
    pub fn new(
        id: impl Into<String>,
        pool: PgPool,
        clock: Arc<dyn Clock>,
        config: SchedulerConfig,
    ) -> Self {
        Self {
            id: id.into(),
            pool,
            clock,
            config,
            chores: Vec::new(),
        }
    }

    pub fn with_chore(mut self, chore: Arc<dyn Chore>) -> Self {
        self.chores.push(chore);
        self
    }

    /// Runs until `shutdown` fires, then gives up leadership so another scheduler takes over
    /// at once. A database that fails is logged and retried on the next poll.
    pub async fn run(&self, shutdown: CancellationToken) -> Result<(), EngineError> {
        let span = tracing::info_span!("scheduler", scheduler = %self.id);
        async {
            tracing::info!("scheduler started");
            let mut lock = LeaderLock::new(&self.pool);
            // When each Chore runs next; set when this scheduler becomes leader.
            let mut chores_due: Option<Vec<DateTime<Utc>>> = None;
            loop {
                match lock.try_hold().await {
                    Ok(true) => {
                        let due = chores_due.get_or_insert_with(|| {
                            tracing::info!("leading");
                            vec![self.clock.now(); self.chores.len()]
                        });
                        if let Err(error) = self.tick().await {
                            tracing::error!(%error, "cannot fire schedules; retrying on the next poll");
                        }
                        self.run_chores(due).await;
                    }
                    Ok(false) => {
                        if chores_due.take().is_some() {
                            tracing::warn!("no longer leading");
                        }
                    }
                    Err(error) => {
                        if chores_due.take().is_some() {
                            tracing::warn!(%error, "lost the leader lock's connection");
                        } else {
                            tracing::warn!(%error, "cannot reach the leader lock");
                        }
                    }
                }

                let poll = self.clock.now() + self.config.poll_interval;
                tokio::select! {
                    biased;
                    () = shutdown.cancelled() => break,
                    () = self.clock.sleep_until(poll) => {}
                }
            }
            if let Err(error) = lock.release().await {
                tracing::warn!(%error, "cannot release the leader lock; it goes with the connection");
            }
            tracing::info!("scheduler stopped");
            Ok(())
        }
        .instrument(span)
        .await
    }

    /// Rebuilds the Schedules of Workflows whose Active Version changed, then fires every
    /// Schedule due at the Clock's current time. Returns the Runs it started. Safe to call
    /// from several schedulers at once, leading or not.
    pub async fn tick(&self) -> Result<Vec<RunId>, EngineError> {
        let now = self.clock.now();
        self.sync(now).await?;
        let mut started = Vec::new();
        for _ in 0..FIRES_PER_POLL {
            match self.fire_next(now).await? {
                Some(runs) => started.extend(runs),
                None => break,
            }
        }
        Ok(started)
    }

    async fn run_chores(&self, due: &mut [DateTime<Utc>]) {
        for (chore, due) in self.chores.iter().zip(due) {
            let now = self.clock.now();
            if *due > now {
                continue;
            }
            let claim = match self.pool.acquire().await {
                Ok(mut conn) => {
                    PgChoreRepository
                        .claim(&mut conn, chore.name(), now, chore.interval())
                        .await
                }
                Err(error) => Err(error.into()),
            };
            match claim {
                Ok(ChoreClaim::Claimed) => *due = now + chore.interval(),
                Ok(ChoreClaim::RanAt(ran_at)) => {
                    *due = ran_at + chore.interval();
                    continue;
                }
                Err(error) => {
                    tracing::error!(chore = chore.name(), %error, "cannot claim chore; retrying on the next poll");
                    continue;
                }
            }
            let span = tracing::info_span!("chore", chore = chore.name());
            if let Err(error) = chore.run(&self.pool, now).instrument(span).await {
                tracing::error!(chore = chore.name(), %error, "chore failed");
            }
        }
    }

    async fn sync(&self, now: DateTime<Utc>) -> Result<(), EngineError> {
        let mut conn = self.pool.acquire().await?;
        let stale = PgScheduleRepository.out_of_sync(&mut conn).await?;
        drop(conn);
        for workflow in stale {
            if let Err(error) = self.sync_workflow(&workflow, now).await {
                tracing::error!(workflow_id = %workflow, %error, "cannot rebuild schedules");
            }
        }
        Ok(())
    }

    /// Replaces the Workflow's Schedules with those of its Active Version's cron Triggers. A
    /// Trigger whose timing did not change keeps its next occurrence and queue; one whose
    /// timing changed starts from the next occurrence after `now`. Both keep their last Run,
    /// so the Overlap Policy still sees a Run the previous Version started.
    async fn sync_workflow(
        &self,
        workflow: &WorkflowId,
        now: DateTime<Utc>,
    ) -> Result<(), EngineError> {
        let mut tx = self.pool.begin().await?;
        let Some(state) = PgScheduleRepository
            .lock_for_sync(&mut tx, workflow)
            .await?
        else {
            return Ok(());
        };
        if state.active == state.scheduled {
            return Ok(());
        }
        let existing = PgScheduleRepository
            .list_for_workflow(&mut tx, workflow)
            .await?;
        let mut schedules = Vec::new();
        if let Some(version) = state.active {
            let definition = PgVersionRepository
                .get(&mut tx, workflow, version)
                .await?
                .ok_or_else(|| StoreError::VersionNotFound {
                    workflow: workflow.clone(),
                    number: version,
                })?
                .definition;
            for trigger in definition.triggers {
                let TriggerKind::Cron(cron) = trigger.kind else {
                    continue;
                };
                let previous = existing
                    .iter()
                    .find(|schedule| schedule.trigger_id == trigger.id);
                let schedule = Schedule {
                    workflow_id: workflow.clone(),
                    trigger_id: trigger.id,
                    version,
                    trigger: CronTrigger {
                        overlap: Some(cron.overlap_policy()),
                        catch_up: Some(cron.catch_up_policy()),
                        ..cron
                    },
                    next_at: now,
                    due_at: now,
                    queued: Vec::new(),
                    last_run: previous.and_then(|previous| previous.last_run),
                };
                if let Some(schedule) = rebuilt(schedule, previous, now) {
                    schedules.push(schedule);
                }
            }
        }
        PgScheduleRepository
            .replace(&mut tx, workflow, state.active, &schedules)
            .await?;
        tx.commit().await?;
        tracing::info!(
            workflow_id = %workflow,
            version = state.active.map(VersionNumber::get),
            schedules = schedules.len(),
            "schedules rebuilt"
        );
        Ok(())
    }

    /// Claims one due Schedule and fires what its policies say. `None` when nothing is due.
    async fn fire_next(&self, now: DateTime<Utc>) -> Result<Option<Vec<RunId>>, EngineError> {
        let mut tx = self.pool.begin().await?;
        let Some(due) = PgScheduleRepository.claim_due(&mut tx, now).await? else {
            return Ok(None);
        };
        let span = tracing::info_span!(
            "schedule",
            workflow_id = %due.schedule.workflow_id,
            trigger_id = %due.schedule.trigger_id,
        );
        let started = fire(&mut tx, due, now).instrument(span.clone()).await?;
        tx.commit().await?;
        let _entered = span.enter();
        for (at, run) in &started {
            tracing::info!(run_id = %run, scheduled_at = %rfc3339(*at), "schedule fired");
        }
        Ok(Some(started.into_iter().map(|(_, run)| run).collect()))
    }
}

/// Fires a claimed Schedule inside the claiming transaction and writes it back. Returns the
/// Runs it started and the occurrence each one is for.
async fn fire(
    tx: &mut PgConnection,
    due: DueSchedule,
    now: DateTime<Utc>,
) -> Result<Vec<(DateTime<Utc>, RunId)>, EngineError> {
    let DueSchedule {
        mut schedule,
        previous_active,
    } = due;
    let cron = match schedule.trigger.cron() {
        Ok(cron) => cron,
        Err(error) => {
            tracing::error!(%error, "schedule cannot be read; deleting it");
            PgScheduleRepository.delete(tx, &schedule).await?;
            return Ok(Vec::new());
        }
    };
    let plan = plan(&schedule, &cron, previous_active, now);

    let mut started = Vec::new();
    for &at in &plan.fire {
        let run = start_run(
            tx,
            &schedule.workflow_id,
            schedule.version,
            json!({}),
            trigger_metadata(&schedule, at),
            now,
        )
        .await?;
        started.push((at, run.id));
        schedule.last_run = Some(run.id);
    }
    schedule.queued = plan.queued;
    match plan.next_at {
        Some(next_at) => {
            schedule.next_at = next_at;
            schedule.due_at = due_at(&schedule, next_at);
            PgScheduleRepository.save(tx, &schedule).await?;
        }
        None => {
            tracing::warn!("cron expression never fires again; deleting its schedule");
            PgScheduleRepository.delete(tx, &schedule).await?;
        }
    }

    for at in &plan.skipped {
        tracing::info!(scheduled_at = %rfc3339(*at), "previous run still active; skipped");
    }
    if plan.missed > 0 || plan.missed_more {
        tracing::warn!(
            missed = plan.missed,
            older_dropped = plan.missed_more,
            "occurrences missed while no scheduler ran were not fired"
        );
    }
    Ok(started)
}

/// The Schedule for a Trigger of the new Active Version. `None` when its expression never
/// fires again.
fn rebuilt(
    mut schedule: Schedule,
    previous: Option<&Schedule>,
    now: DateTime<Utc>,
) -> Option<Schedule> {
    let same_timing = previous.filter(|previous| {
        let (old, new) = (&previous.trigger, &schedule.trigger);
        old.expr == new.expr && old.tz == new.tz && old.jitter == new.jitter
    });
    if let Some(previous) = same_timing {
        schedule.next_at = previous.next_at;
        schedule.due_at = previous.due_at;
        schedule.queued = previous.queued.clone();
        return Some(schedule);
    }
    // Validation rejects such an expression at publish, so this only meets one published by
    // an older release that did not check it.
    let cron = match schedule.trigger.cron() {
        Ok(cron) => cron,
        Err(error) => {
            tracing::error!(
                workflow_id = %schedule.workflow_id,
                trigger_id = %schedule.trigger_id,
                %error,
                "cron trigger cannot be scheduled"
            );
            return None;
        }
    };
    schedule.next_at = cron.next_after(now)?;
    schedule.due_at = due_at(&schedule, schedule.next_at);
    Some(schedule)
}

/// The `trigger` of a Run a Schedule starts.
fn trigger_metadata(schedule: &Schedule, at: DateTime<Utc>) -> Value {
    json!({
        "type": "cron",
        "id": schedule.trigger_id.as_str(),
        "scheduled_at": rfc3339(at),
    })
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}
