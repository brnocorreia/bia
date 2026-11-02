use bia_core::dsl::TriggerId;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{Origin, Scope, Token, TokenId};
use crate::StoreError;
use crate::user::UserId;
use crate::workflow::workflow_id;

#[derive(Debug)]
pub(crate) struct TokenRow {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) scope: String,
    pub(crate) origin: String,
    pub(crate) workflow_id: Option<String>,
    pub(crate) trigger_id: Option<String>,
    pub(crate) user_id: Uuid,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) last_used_at: Option<DateTime<Utc>>,
    pub(crate) revoked_at: Option<DateTime<Utc>>,
}

impl TryFrom<TokenRow> for Token {
    type Error = StoreError;

    fn try_from(row: TokenRow) -> Result<Self, StoreError> {
        let scope = match (row.scope.as_str(), row.workflow_id, row.trigger_id) {
            ("admin", None, None) => Scope::Admin,
            ("agent", None, None) => Scope::Agent,
            ("webhook", Some(workflow), Some(trigger)) => Scope::Webhook {
                workflow_id: workflow_id(workflow)?,
                trigger_id: TriggerId::new(trigger)
                    .map_err(|err| StoreError::invalid("trigger id", err))?,
            },
            (other, _, _) => return Err(StoreError::invalid("token scope", other)),
        };
        let origin = match row.origin.as_str() {
            "session" => Origin::Session,
            "oauth" => Origin::OAuth,
            "manual" => Origin::Manual,
            other => return Err(StoreError::invalid("token origin", other)),
        };
        Ok(Self {
            id: TokenId::from_uuid(row.id),
            name: row.name,
            scope,
            origin,
            user_id: UserId::from_uuid(row.user_id),
            created_at: row.created_at,
            last_used_at: row.last_used_at,
            revoked_at: row.revoked_at,
        })
    }
}
