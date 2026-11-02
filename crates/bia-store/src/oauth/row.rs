use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{Authorization, Client, DeviceAuthorization, Grant, GrantScope, Provenance};
use crate::StoreError;
use crate::user::UserId;

fn provenance(value: &str) -> Result<Provenance, StoreError> {
    match value {
        "cimd" => Ok(Provenance::Cimd),
        "dynamic" => Ok(Provenance::Dynamic),
        "builtin" => Ok(Provenance::Builtin),
        other => Err(StoreError::invalid("grant provenance", other)),
    }
}

fn scope(value: &str) -> Result<GrantScope, StoreError> {
    match value {
        "admin" => Ok(GrantScope::Admin),
        "agent" => Ok(GrantScope::Agent),
        other => Err(StoreError::invalid("grant scope", other)),
    }
}

#[derive(Debug)]
pub(crate) struct ClientRow {
    pub(crate) client_id: String,
    pub(crate) client_name: String,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) created_at: DateTime<Utc>,
}

impl From<ClientRow> for Client {
    fn from(row: ClientRow) -> Self {
        Self {
            client_id: row.client_id,
            client_name: row.client_name,
            redirect_uris: row.redirect_uris,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug)]
pub(crate) struct AuthorizationRow {
    pub(crate) id: Uuid,
    pub(crate) client_id: String,
    pub(crate) client_name: String,
    pub(crate) client_host: Option<String>,
    pub(crate) provenance: String,
    pub(crate) redirect_uri: String,
    pub(crate) requested_scope: String,
    pub(crate) resource: String,
    pub(crate) state: Option<String>,
    pub(crate) code_challenge: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) expires_at: DateTime<Utc>,
    pub(crate) user_id: Option<Uuid>,
    pub(crate) granted_scope: Option<String>,
    pub(crate) code_expires_at: Option<DateTime<Utc>>,
    pub(crate) grant_id: Option<Uuid>,
}

impl TryFrom<AuthorizationRow> for Authorization {
    type Error = StoreError;

    fn try_from(row: AuthorizationRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.id,
            client_id: row.client_id,
            client_name: row.client_name,
            client_host: row.client_host,
            provenance: provenance(&row.provenance)?,
            redirect_uri: row.redirect_uri,
            requested_scope: scope(&row.requested_scope)?,
            resource: row.resource,
            state: row.state,
            code_challenge: row.code_challenge,
            created_at: row.created_at,
            expires_at: row.expires_at,
            user_id: row.user_id.map(UserId::from_uuid),
            granted_scope: row.granted_scope.as_deref().map(scope).transpose()?,
            code_expires_at: row.code_expires_at,
            grant_id: row.grant_id,
        })
    }
}

#[derive(Debug)]
pub(crate) struct GrantRow {
    pub(crate) id: Uuid,
    pub(crate) user_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) client_name: String,
    pub(crate) client_host: Option<String>,
    pub(crate) provenance: String,
    pub(crate) scope: String,
    pub(crate) resource: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) last_used_at: Option<DateTime<Utc>>,
    pub(crate) revoked_at: Option<DateTime<Utc>>,
}

impl TryFrom<GrantRow> for Grant {
    type Error = StoreError;

    fn try_from(row: GrantRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.id,
            user_id: UserId::from_uuid(row.user_id),
            client_id: row.client_id,
            client_name: row.client_name,
            client_host: row.client_host,
            provenance: provenance(&row.provenance)?,
            scope: scope(&row.scope)?,
            resource: row.resource,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
            revoked_at: row.revoked_at,
        })
    }
}

#[derive(Debug)]
pub(crate) struct DeviceAuthorizationRow {
    pub(crate) id: Uuid,
    pub(crate) client_id: String,
    pub(crate) client_name: String,
    pub(crate) client_host: Option<String>,
    pub(crate) provenance: String,
    pub(crate) requested_scope: String,
    pub(crate) resource: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) expires_at: DateTime<Utc>,
    pub(crate) interval_secs: i32,
    pub(crate) user_id: Option<Uuid>,
    pub(crate) granted_scope: Option<String>,
    pub(crate) grant_id: Option<Uuid>,
}

impl TryFrom<DeviceAuthorizationRow> for DeviceAuthorization {
    type Error = StoreError;

    fn try_from(row: DeviceAuthorizationRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.id,
            client_id: row.client_id,
            client_name: row.client_name,
            client_host: row.client_host,
            provenance: provenance(&row.provenance)?,
            requested_scope: scope(&row.requested_scope)?,
            resource: row.resource,
            created_at: row.created_at,
            expires_at: row.expires_at,
            interval_secs: row.interval_secs,
            user_id: row.user_id.map(UserId::from_uuid),
            granted_scope: row.granted_scope.as_deref().map(scope).transpose()?,
            grant_id: row.grant_id,
        })
    }
}
