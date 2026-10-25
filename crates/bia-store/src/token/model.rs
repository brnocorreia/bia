use std::fmt;

use bia_core::dsl::{TriggerId, WorkflowId};
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TokenId(Uuid);

impl TokenId {
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The SHA-256 of a Token's value, which is what identifies a Token in the database.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TokenHash(pub [u8; 32]);

impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenHash(..)")
    }
}

/// What a Token may do.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Everything.
    Admin,
    /// Author Drafts, validate, dry-run, trigger and read; never publish or activate.
    Agent,
    /// Start Runs from this one webhook Trigger.
    Webhook {
        workflow_id: WorkflowId,
        trigger_id: TriggerId,
    },
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Agent => "agent",
            Self::Webhook { .. } => "webhook",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub id: TokenId,
    pub name: String,
    pub scope: Scope,
    pub created_at: DateTime<Utc>,
    /// When the Token last authenticated a request, to the minute: recording every use would
    /// write on every request.
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewToken {
    pub name: String,
    pub hash: TokenHash,
    pub scope: Scope,
    pub created_at: DateTime<Utc>,
}
