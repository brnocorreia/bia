use bia_core::dsl::Definition;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{Version, VersionNumber, VersionState};
use crate::StoreError;
use crate::workflow::workflow_id;

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
            workflow_id: workflow_id(row.workflow_id)?,
            number: version_number(row.number)?,
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

pub(crate) fn version_number(number: i32) -> Result<VersionNumber, StoreError> {
    VersionNumber::new(number).ok_or_else(|| StoreError::invalid("version number", number))
}
