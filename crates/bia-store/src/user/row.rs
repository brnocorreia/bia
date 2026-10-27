use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{User, UserId};

#[derive(Debug)]
pub(crate) struct UserRow {
    pub(crate) id: Uuid,
    pub(crate) created_at: DateTime<Utc>,
}

impl From<UserRow> for User {
    fn from(row: UserRow) -> Self {
        Self {
            id: UserId::from_uuid(row.id),
            created_at: row.created_at,
        }
    }
}
