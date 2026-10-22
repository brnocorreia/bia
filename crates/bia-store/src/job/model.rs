use std::fmt;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

use crate::run::RunId;

/// How many Attempts a Job gets unless its enqueuer says otherwise.
pub const DEFAULT_MAX_ATTEMPTS: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(i64);

impl JobId {
    pub fn new(id: i64) -> Self {
        Self(id)
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What a Job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JobKind {
    AdvanceRun,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdvanceRun => "advance_run",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewJob {
    pub kind: JobKind,
    pub run_id: Option<RunId>,
    pub payload: Value,
    /// When the Job becomes claimable; later than now for a delayed Job.
    pub run_at: DateTime<Utc>,
    /// How many Attempts the Job gets before it gives up; at least 1.
    pub max_attempts: i32,
}

impl NewJob {
    /// A Job that advances `run`, claimable from `run_at`.
    pub fn advance_run(run: RunId, run_at: DateTime<Utc>) -> Self {
        Self {
            kind: JobKind::AdvanceRun,
            run_id: Some(run),
            payload: Value::Object(serde_json::Map::new()),
            run_at,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub run_id: Option<RunId>,
    pub payload: Value,
    /// When the Job is next claimable.
    pub run_at: DateTime<Utc>,
    /// How many times the Job has been claimed, including claims whose Lease expired.
    pub attempts: i32,
    pub max_attempts: i32,
    pub locked_by: Option<String>,
    pub locked_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// When the Job gave up after its last Attempt failed.
    pub failed_at: Option<DateTime<Utc>>,
}

/// A time-bounded claim on a Job. Only the claim that produced it can ack, fail or extend it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    job_id: JobId,
    attempt: i32,
    until: DateTime<Utc>,
}

impl Lease {
    pub(crate) fn new(job_id: JobId, attempt: i32, until: DateTime<Utc>) -> Self {
        Self {
            job_id,
            attempt,
            until,
        }
    }

    pub fn job_id(&self) -> JobId {
        self.job_id
    }

    /// Which claim of the Job this Lease belongs to, starting at 1.
    pub fn attempt(&self) -> i32 {
        self.attempt
    }

    /// When the Lease expires and the Job becomes claimable again.
    pub fn until(&self) -> DateTime<Utc> {
        self.until
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Claimed {
    pub job: Job,
    pub lease: Lease,
}

/// How long a failed Job waits before its next Attempt: `initial` doubled after each Attempt,
/// capped at `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub initial: TimeDelta,
    pub max: TimeDelta,
}

impl Backoff {
    /// The delay after Attempt `attempt` (1-based) fails.
    pub fn delay(&self, attempt: i32) -> TimeDelta {
        let doublings = u32::try_from(attempt.saturating_sub(1)).unwrap_or(0);
        2_i32
            .checked_pow(doublings)
            .and_then(|factor| self.initial.checked_mul(factor))
            .map_or(self.max, |delay| delay.min(self.max))
    }
}

impl Default for Backoff {
    /// 1 s, 2 s, 4 s… up to an hour.
    fn default() -> Self {
        Self {
            initial: TimeDelta::seconds(1),
            max: TimeDelta::hours(1),
        }
    }
}

/// What failing a Job led to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failed {
    /// The Job is claimable again from `run_at`.
    Retrying { run_at: DateTime<Utc> },
    /// The Job used its last Attempt and will not run again.
    GaveUp,
}
