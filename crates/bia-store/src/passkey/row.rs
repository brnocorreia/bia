use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{Passkey, PasskeyId};
use crate::user::UserId;

#[derive(Debug)]
pub(crate) struct PasskeyRow {
    pub(crate) id: Uuid,
    pub(crate) user_id: Uuid,
    pub(crate) name: String,
    pub(crate) credential: serde_json::Value,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) last_used_at: Option<DateTime<Utc>>,
}

impl From<PasskeyRow> for Passkey {
    fn from(row: PasskeyRow) -> Self {
        Self {
            id: PasskeyId::from_uuid(row.id),
            user_id: UserId::from_uuid(row.user_id),
            name: row.name,
            credential: row.credential,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
        }
    }
}
