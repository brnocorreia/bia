use bia_core::dsl::Definition;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::StoreError;
use crate::models::{Version, VersionState};

#[derive(Debug)]
pub(crate) struct VersionRow {
    pub(crate) workflow_id: String,
    pub(crate) number: i32,
    pub(crate) definition: Value,
    pub(crate) state: String,
    pub(crate) published_at: Option<DateTime<Utc>>,
    pub(crate) created_at: DateTime<Utc>,
}

impl TryFrom<VersionRow> for Version {
    type Error = StoreError;

    fn try_from(row: VersionRow) -> Result<Self, StoreError> {
        Ok(Self {
            workflow_id: super::workflow_id(row.workflow_id)?,
            number: super::version_number(row.number)?,
            definition: Definition::from_value(row.definition)
                .map_err(|err| StoreError::invalid("definition", err))?,
            state: version_state(&row.state)?,
            published_at: row.published_at,
            created_at: row.created_at,
        })
    }
}

pub(crate) fn version_state(state: &str) -> Result<VersionState, StoreError> {
    match state {
        "draft" => Ok(VersionState::Draft),
        "published" => Ok(VersionState::Published),
        other => Err(StoreError::invalid("version state", other)),
    }
}
