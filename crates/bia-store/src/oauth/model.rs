use std::fmt;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::token::Scope;
use crate::user::UserId;

/// The SHA-256 of an authorization code or refresh token.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SecretHash(pub [u8; 32]);

impl fmt::Debug for SecretHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretHash(..)")
    }
}

/// What a Grant may do: the Scopes a person can approve for a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GrantScope {
    Admin,
    Agent,
}

impl GrantScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Agent => "agent",
        }
    }

    pub fn scope(self) -> Scope {
        match self {
            Self::Admin => Scope::Admin,
            Self::Agent => Scope::Agent,
        }
    }

    /// Whether a Grant of `self` may be narrowed to `other` or kept: `admin` covers `agent`.
    pub fn covers(self, other: Self) -> bool {
        self == Self::Admin || other == Self::Agent
    }
}

/// Where a client's identity comes from, which the consent screen tells the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provenance {
    /// A Client ID Metadata Document fetched from the host in `client_host`.
    Cimd,
    /// Registered itself with `POST /oauth/register`; nothing vouches for it.
    Dynamic,
    /// Shipped with bia, like the CLI.
    Builtin,
}

impl Provenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cimd => "cimd",
            Self::Dynamic => "dynamic",
            Self::Builtin => "builtin",
        }
    }
}

/// A client that registered itself with Dynamic Client Registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewClient {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// An authorization request waiting for a person, then the code it produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorization {
    pub id: Uuid,
    pub client_id: String,
    pub client_name: String,
    pub client_host: Option<String>,
    pub provenance: Provenance,
    pub redirect_uri: String,
    pub requested_scope: GrantScope,
    pub resource: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub user_id: Option<UserId>,
    pub granted_scope: Option<GrantScope>,
    pub code_expires_at: Option<DateTime<Utc>>,
    pub grant_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct NewAuthorization {
    pub client_id: String,
    pub client_name: String,
    pub client_host: Option<String>,
    pub provenance: Provenance,
    pub redirect_uri: String,
    pub requested_scope: GrantScope,
    pub resource: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub id: Uuid,
    pub user_id: UserId,
    pub client_id: String,
    pub client_name: String,
    pub client_host: Option<String>,
    pub provenance: Provenance,
    pub scope: GrantScope,
    pub resource: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewGrant {
    pub user_id: UserId,
    pub client_id: String,
    pub client_name: String,
    pub client_host: Option<String>,
    pub provenance: Provenance,
    pub scope: GrantScope,
    pub resource: String,
    pub created_at: DateTime<Utc>,
}

/// What presenting an authorization code finds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeRedemption {
    /// First use of an approved code; it is spent now whatever happens next.
    Redeemed(Box<Authorization>),
    /// No approved authorization has this code.
    Unknown,
    /// The code was used before. The Grant it produced, if any, has been revoked.
    Replayed,
}

/// What presenting a refresh token finds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshRedemption {
    /// First use of a live refresh token on a live Grant; it is spent now.
    Redeemed(Grant),
    /// No refresh token has this hash.
    Unknown,
    /// Past its 90 days.
    Expired,
    /// The Grant is revoked.
    Revoked,
    /// A spent refresh token came back, so it leaked: the Grant is revoked, with everything it
    /// issued.
    Reused(Grant),
}

/// A person's answer to a pending authorization: who approved, with what Scope, and the hash of
/// the code that redeems it.
#[derive(Debug, Clone)]
pub struct NewApproval {
    pub user_id: UserId,
    pub scope: GrantScope,
    pub code: SecretHash,
    pub now: DateTime<Utc>,
    pub code_expires_at: DateTime<Utc>,
}
