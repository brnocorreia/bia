use bia_core::dsl::{CatchUpPolicy, CronTrigger, OverlapPolicy, TriggerId};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::Schedule;
use crate::StoreError;
use crate::run::RunId;
use crate::version::version_number;
use crate::workflow::workflow_id;

#[derive(Debug)]
pub(crate) struct ScheduleRow {
    pub(crate) workflow_id: String,
    pub(crate) trigger_id: String,
    pub(crate) version: i32,
    pub(crate) expr: String,
    pub(crate) tz: Option<String>,
    pub(crate) jitter: Option<String>,
    pub(crate) overlap: String,
    pub(crate) catch_up: String,
    pub(crate) next_at: DateTime<Utc>,
    pub(crate) due_at: DateTime<Utc>,
    pub(crate) queued: Vec<DateTime<Utc>>,
    pub(crate) last_run_id: Option<Uuid>,
}

impl TryFrom<ScheduleRow> for Schedule {
    type Error = StoreError;

    fn try_from(row: ScheduleRow) -> Result<Self, StoreError> {
        let overlap = match row.overlap.as_str() {
            "skip" => OverlapPolicy::Skip,
            "queue" => OverlapPolicy::Queue,
            "allow" => OverlapPolicy::Allow,
            other => return Err(StoreError::invalid("overlap policy", other)),
        };
        let catch_up = match row.catch_up.as_str() {
            "none" => CatchUpPolicy::None,
            "latest" => CatchUpPolicy::Latest,
            "all" => CatchUpPolicy::All,
            other => return Err(StoreError::invalid("catch-up policy", other)),
        };
        let jitter = row
            .jitter
            .map(|jitter| jitter.parse())
            .transpose()
            .map_err(|err| StoreError::invalid("jitter", err))?;
        Ok(Self {
            workflow_id: workflow_id(row.workflow_id)?,
            trigger_id: TriggerId::new(row.trigger_id)
                .map_err(|err| StoreError::invalid("trigger id", err))?,
            version: version_number(row.version)?,
            trigger: CronTrigger {
                expr: row.expr,
                tz: row.tz,
                jitter,
                overlap: Some(overlap),
                catch_up: Some(catch_up),
            },
            next_at: row.next_at,
            due_at: row.due_at,
            queued: row.queued,
            last_run: row.last_run_id.map(RunId::from_uuid),
        })
    }
}
