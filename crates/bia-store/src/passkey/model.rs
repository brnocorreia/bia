use std::fmt;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::user::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PasskeyId(Uuid);

impl PasskeyId {
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for PasskeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CeremonyId(Uuid);

impl CeremonyId {
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Passkey {
    pub id: PasskeyId,
    pub user_id: UserId,
    pub name: String,
    /// The serialised credential, public data only.
    pub credential: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewPasskey {
    pub user_id: UserId,
    pub name: String,
    pub credential: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeremonyKind {
    Registration,
    Authentication,
}

impl CeremonyKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Registration => "registration",
            Self::Authentication => "authentication",
        }
    }
}
