use std::fmt;

use bia_core::dsl::{TriggerId, WorkflowId};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::user::UserId;

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

/// Where a Token came from (ADR-0064).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A browser sign-in. The only origin that expires.
    Session,
    /// Granted to an OAuth client.
    OAuth,
    /// Created by hand, for CI and scripts.
    Manual,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::OAuth => "oauth",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub id: TokenId,
    pub name: String,
    pub scope: Scope,
    pub origin: Origin,
    pub user_id: UserId,
    pub created_at: DateTime<Utc>,
    /// When the Token last authenticated a request, to the minute: recording every use would
    /// write on every request.
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// The browser a Session was started from, as it described itself.
    pub user_agent: Option<String>,
    /// The OAuth Grant an `oauth` Token belongs to.
    pub grant_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct NewToken {
    pub name: String,
    pub hash: TokenHash,
    pub scope: Scope,
    pub origin: Origin,
    /// Who the Token acts for.
    pub user_id: UserId,
    pub created_at: DateTime<Utc>,
    pub user_agent: Option<String>,
    pub grant_id: Option<Uuid>,
}

/// A Session unused for this long stops authenticating.
pub const SESSION_IDLE_LIFETIME: chrono::TimeDelta = chrono::TimeDelta::days(14);
/// A Session older than this stops authenticating, however often it is used.
pub const SESSION_MAX_LIFETIME: chrono::TimeDelta = chrono::TimeDelta::days(90);

impl Token {
    /// Whether a Session has outlived its lifetime at `now`. Tokens of other origins never
    /// expire. Use is recorded to the minute, so the idle limit is judged to the minute too.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.origin == Origin::Session
            && (now >= self.created_at + SESSION_MAX_LIFETIME
                || now >= self.last_used_at.unwrap_or(self.created_at) + SESSION_IDLE_LIFETIME)
    }
}
