//! Timelines: the ordered record of a Run, one entry per Attempt.
//!
//! Entries reach operators and agents verbatim, so callers must keep resolved Secrets out of
//! inputs, Outputs and errors.

use bia_core::dsl::{ActionName, StepId};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;
use sqlx::{Acquire, PgConnection};

use crate::StoreError;
use crate::runs::RunId;

/// How an Attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptStatus {
    Succeeded,
    Failed,
    /// The Step's `if` was false, so its Action never ran.
    Skipped,
}

impl AttemptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    fn from_db(status: &str) -> Result<Self, StoreError> {
        Ok(match status {
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            other => return Err(StoreError::invalid("attempt status", other)),
        })
    }
}

/// One Attempt to record.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTimelineEntry {
    pub step_id: StepId,
    /// Which iteration of the enclosing loops, as in the Idempotency Key; empty outside loops.
    pub iteration_path: String,
    /// 1 for the first Attempt, then 2, 3… on retries.
    pub attempt: i32,
    pub action: ActionName,
    pub status: AttemptStatus,
    /// The resolved Params, without Secrets.
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<Value>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

/// One recorded Attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineEntry {
    pub run_id: RunId,
    /// Position in the Run's Timeline: 1, 2, 3…
    pub seq: i32,
    pub step_id: StepId,
    pub iteration_path: String,
    pub attempt: i32,
    pub action: ActionName,
    pub status: AttemptStatus,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<Value>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

impl TimelineEntry {
    /// How long the Attempt took.
    pub fn duration(&self) -> TimeDelta {
        self.finished_at - self.started_at
    }
}

/// A `timeline_entries` row as read back from Postgres.
struct TimelineRow {
    run_id: uuid::Uuid,
    seq: i32,
    step_id: String,
    iteration_path: String,
    attempt: i32,
    action: String,
    status: String,
    input: Option<Value>,
    output: Option<Value>,
    error: Option<Value>,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
}

impl TryFrom<TimelineRow> for TimelineEntry {
    type Error = StoreError;

    fn try_from(row: TimelineRow) -> Result<Self, StoreError> {
        Ok(Self {
            run_id: RunId::from_uuid(row.run_id),
            seq: row.seq,
            step_id: StepId::new(row.step_id).map_err(|err| StoreError::invalid("step id", err))?,
            iteration_path: row.iteration_path,
            attempt: row.attempt,
            action: ActionName::new(row.action)
                .map_err(|err| StoreError::invalid("action name", err))?,
            status: AttemptStatus::from_db(&row.status)?,
            input: row.input,
            output: row.output,
            error: row.error,
            started_at: row.started_at,
            finished_at: row.finished_at,
        })
    }
}

/// Appends an Attempt to the end of a Run's Timeline and returns its `seq`.
pub async fn append(
    conn: &mut PgConnection,
    run: RunId,
    entry: &NewTimelineEntry,
) -> Result<i32, StoreError> {
    let mut tx = conn.begin().await?;

    // The row lock on the Run orders concurrent appends, e.g. from parallel branches.
    let seq = sqlx::query_scalar!(
        "UPDATE runs SET timeline_len = timeline_len + 1 WHERE id = $1 RETURNING timeline_len",
        run.as_uuid(),
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(StoreError::RunNotFound(run))?;

    sqlx::query!(
        r#"
        INSERT INTO timeline_entries (
            run_id, seq, step_id, iteration_path, attempt, action, status,
            input, output, error, started_at, finished_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
        run.as_uuid(),
        seq,
        entry.step_id.as_str(),
        entry.iteration_path,
        entry.attempt,
        entry.action.as_str(),
        entry.status.as_str(),
        entry.input,
        entry.output,
        entry.error,
        entry.started_at,
        entry.finished_at,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(seq)
}

/// Lists a Run's Timeline in order.
pub async fn list(conn: &mut PgConnection, run: RunId) -> Result<Vec<TimelineEntry>, StoreError> {
    let rows = sqlx::query_as!(
        TimelineRow,
        r#"
        SELECT run_id, seq, step_id, iteration_path, attempt, action, status,
               input, output, error, started_at, finished_at
        FROM timeline_entries
        WHERE run_id = $1
        ORDER BY seq
        "#,
        run.as_uuid(),
    )
    .fetch_all(&mut *conn)
    .await?;

    rows.into_iter().map(TimelineEntry::try_from).collect()
}
