use std::future::Future;

use chrono::{DateTime, Utc};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use super::row::{AuthorizationRow, ClientRow, GrantRow};
use super::{
    Authorization, Client, CodeRedemption, Grant, NewApproval, NewAuthorization, NewClient,
    NewGrant, RefreshRedemption, SecretHash,
};
use crate::StoreError;

pub trait OAuthRepository {
    fn register_client(
        &self,
        conn: &mut PgConnection,
        client: &NewClient,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    fn client(
        &self,
        conn: &mut PgConnection,
        client_id: &str,
    ) -> impl Future<Output = Result<Option<Client>, StoreError>> + Send;

    fn begin_authorization(
        &self,
        conn: &mut PgConnection,
        request: &NewAuthorization,
    ) -> impl Future<Output = Result<Authorization, StoreError>> + Send;

    /// The authorization with this id if nobody decided it yet and it has not run out at `now`.
    fn pending_authorization(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<Authorization>, StoreError>> + Send;

    /// Approves a pending authorization for `user` with `scope`, storing the hash of the code
    /// that redeems it until `code_expires_at`. `None` when it is no longer pending, so two
    /// answers to one request cannot both win.
    fn approve(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        approval: &NewApproval,
    ) -> impl Future<Output = Result<Option<Authorization>, StoreError>> + Send;

    /// Denies a pending authorization; `None` when it is no longer pending.
    fn deny(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<Authorization>, StoreError>> + Send;

    /// Spends the code. Presenting a spent code again revokes the Grant it produced.
    fn redeem_code(
        &self,
        conn: &mut PgConnection,
        code: SecretHash,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<CodeRedemption, StoreError>> + Send;

    /// Creates a Grant for flows that have no authorization request, such as the device flow.
    fn create_grant(
        &self,
        conn: &mut PgConnection,
        grant: &NewGrant,
    ) -> impl Future<Output = Result<Grant, StoreError>> + Send;

    /// Creates the Grant a redeemed authorization leads to and remembers it on the
    /// authorization, atomically.
    fn grant_from_authorization(
        &self,
        conn: &mut PgConnection,
        authorization: Uuid,
        grant: &NewGrant,
    ) -> impl Future<Output = Result<Grant, StoreError>> + Send;

    fn grants(
        &self,
        conn: &mut PgConnection,
    ) -> impl Future<Output = Result<Vec<Grant>, StoreError>> + Send;

    fn grant(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<Grant>, StoreError>> + Send;

    /// Revokes the Grant and every access token it issued, keeping the first revocation time;
    /// its refresh tokens stop working with it. `None` when no Grant has this id.
    fn revoke_grant(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<Grant>, StoreError>> + Send;

    fn add_refresh_token(
        &self,
        conn: &mut PgConnection,
        grant: Uuid,
        hash: SecretHash,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Spends the refresh token. Presenting a spent one again revokes its Grant.
    fn redeem_refresh_token(
        &self,
        conn: &mut PgConnection,
        hash: SecretHash,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<RefreshRedemption, StoreError>> + Send;

    /// The Grant an access token or a refresh token belongs to.
    fn grant_of_secret(
        &self,
        conn: &mut PgConnection,
        hash: SecretHash,
    ) -> impl Future<Output = Result<Option<Grant>, StoreError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PgOAuthRepository;

impl OAuthRepository for PgOAuthRepository {
    async fn register_client(
        &self,
        conn: &mut PgConnection,
        client: &NewClient,
    ) -> Result<(), StoreError> {
        sqlx::query_file!(
            "src/oauth/queries/client_insert.sql",
            client.client_id,
            client.client_name,
            &client.redirect_uris,
            client.created_at
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    async fn client(
        &self,
        conn: &mut PgConnection,
        client_id: &str,
    ) -> Result<Option<Client>, StoreError> {
        Ok(
            sqlx::query_file_as!(ClientRow, "src/oauth/queries/client_get.sql", client_id)
                .fetch_optional(&mut *conn)
                .await?
                .map(Client::from),
        )
    }

    async fn begin_authorization(
        &self,
        conn: &mut PgConnection,
        request: &NewAuthorization,
    ) -> Result<Authorization, StoreError> {
        sqlx::query_file_as!(
            AuthorizationRow,
            "src/oauth/queries/authorization_insert.sql",
            request.client_id,
            request.client_name,
            request.client_host,
            request.provenance.as_str(),
            request.redirect_uri,
            request.requested_scope.as_str(),
            request.resource,
            request.state,
            request.code_challenge,
            request.created_at,
            request.expires_at,
        )
        .fetch_one(&mut *conn)
        .await?
        .try_into()
    }

    async fn pending_authorization(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<Authorization>, StoreError> {
        sqlx::query_file_as!(
            AuthorizationRow,
            "src/oauth/queries/authorization_get_pending.sql",
            id,
            now
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(Authorization::try_from)
        .transpose()
    }

    async fn approve(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        approval: &NewApproval,
    ) -> Result<Option<Authorization>, StoreError> {
        sqlx::query_file_as!(
            AuthorizationRow,
            "src/oauth/queries/authorization_approve.sql",
            id,
            approval.now,
            approval.user_id.as_uuid(),
            approval.scope.as_str(),
            &approval.code.0[..],
            approval.code_expires_at
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(Authorization::try_from)
        .transpose()
    }

    async fn deny(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<Authorization>, StoreError> {
        sqlx::query_file_as!(
            AuthorizationRow,
            "src/oauth/queries/authorization_deny.sql",
            id,
            now
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(Authorization::try_from)
        .transpose()
    }

    async fn redeem_code(
        &self,
        conn: &mut PgConnection,
        code: SecretHash,
        now: DateTime<Utc>,
    ) -> Result<CodeRedemption, StoreError> {
        let mut tx = conn.begin().await?;
        let redeemed = sqlx::query_file_as!(
            AuthorizationRow,
            "src/oauth/queries/code_consume.sql",
            &code.0[..],
            now
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = redeemed {
            tx.commit().await?;
            return Ok(CodeRedemption::Redeemed(Box::new(row.try_into()?)));
        }
        let spent = sqlx::query_file!("src/oauth/queries/code_spent.sql", &code.0[..])
            .fetch_optional(&mut *tx)
            .await?;
        let outcome = match spent {
            Some(row) if row.code_used_at.is_some() => {
                if let Some(grant) = row.grant_id {
                    revoke(&mut tx, grant, now).await?;
                }
                CodeRedemption::Replayed
            }
            _ => CodeRedemption::Unknown,
        };
        tx.commit().await?;
        Ok(outcome)
    }

    async fn create_grant(
        &self,
        conn: &mut PgConnection,
        grant: &NewGrant,
    ) -> Result<Grant, StoreError> {
        insert_grant(conn, grant).await
    }

    async fn grant_from_authorization(
        &self,
        conn: &mut PgConnection,
        authorization: Uuid,
        grant: &NewGrant,
    ) -> Result<Grant, StoreError> {
        let mut tx = conn.begin().await?;
        let created = insert_grant(&mut tx, grant).await?;
        sqlx::query_file!(
            "src/oauth/queries/authorization_link_grant.sql",
            authorization,
            created.id
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(created)
    }

    async fn grants(&self, conn: &mut PgConnection) -> Result<Vec<Grant>, StoreError> {
        sqlx::query_file_as!(GrantRow, "src/oauth/queries/grant_list.sql")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(Grant::try_from)
            .collect()
    }

    async fn grant(&self, conn: &mut PgConnection, id: Uuid) -> Result<Option<Grant>, StoreError> {
        sqlx::query_file_as!(GrantRow, "src/oauth/queries/grant_get.sql", id)
            .fetch_optional(&mut *conn)
            .await?
            .map(Grant::try_from)
            .transpose()
    }

    async fn revoke_grant(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<Grant>, StoreError> {
        let mut tx = conn.begin().await?;
        let grant = revoke(&mut tx, id, now).await?;
        tx.commit().await?;
        Ok(grant)
    }

    async fn add_refresh_token(
        &self,
        conn: &mut PgConnection,
        grant: Uuid,
        hash: SecretHash,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        sqlx::query_file!(
            "src/oauth/queries/refresh_insert.sql",
            grant,
            &hash.0[..],
            now,
            expires_at
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    async fn redeem_refresh_token(
        &self,
        conn: &mut PgConnection,
        hash: SecretHash,
        now: DateTime<Utc>,
    ) -> Result<RefreshRedemption, StoreError> {
        let mut tx = conn.begin().await?;
        let redeemed =
            sqlx::query_file_scalar!("src/oauth/queries/refresh_redeem.sql", &hash.0[..], now)
                .fetch_optional(&mut *tx)
                .await?;
        let outcome = if let Some(grant_id) = redeemed {
            match fetch(&mut tx, grant_id).await? {
                Some(grant) if grant.revoked_at.is_none() => RefreshRedemption::Redeemed(grant),
                _ => RefreshRedemption::Revoked,
            }
        } else {
            match sqlx::query_file!("src/oauth/queries/refresh_lookup.sql", &hash.0[..])
                .fetch_optional(&mut *tx)
                .await?
            {
                None => RefreshRedemption::Unknown,
                Some(row) if row.used_at.is_some() => match fetch(&mut tx, row.grant_id).await? {
                    Some(grant) if grant.revoked_at.is_none() => {
                        let revoked = revoke(&mut tx, grant.id, now).await?;
                        RefreshRedemption::Reused(revoked.unwrap_or(grant))
                    }
                    _ => RefreshRedemption::Revoked,
                },
                Some(row) => match fetch(&mut tx, row.grant_id).await? {
                    Some(grant) if grant.revoked_at.is_some() => RefreshRedemption::Revoked,
                    _ => RefreshRedemption::Expired,
                },
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }

    async fn grant_of_secret(
        &self,
        conn: &mut PgConnection,
        hash: SecretHash,
    ) -> Result<Option<Grant>, StoreError> {
        let Some(id) =
            sqlx::query_file_scalar!("src/oauth/queries/grant_of_secret.sql", &hash.0[..])
                .fetch_optional(&mut *conn)
                .await?
        else {
            return Ok(None);
        };
        fetch(conn, id).await
    }
}

async fn fetch(conn: &mut PgConnection, id: Uuid) -> Result<Option<Grant>, StoreError> {
    sqlx::query_file_as!(GrantRow, "src/oauth/queries/grant_get.sql", id)
        .fetch_optional(&mut *conn)
        .await?
        .map(Grant::try_from)
        .transpose()
}

async fn revoke(
    conn: &mut PgConnection,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<Grant>, StoreError> {
    let grant = sqlx::query_file_as!(GrantRow, "src/oauth/queries/grant_revoke.sql", id, now)
        .fetch_optional(&mut *conn)
        .await?
        .map(Grant::try_from)
        .transpose()?;
    if grant.is_some() {
        sqlx::query_file!("src/oauth/queries/grant_revoke_tokens.sql", id, now)
            .execute(&mut *conn)
            .await?;
    }
    Ok(grant)
}

async fn insert_grant(conn: &mut PgConnection, grant: &NewGrant) -> Result<Grant, StoreError> {
    sqlx::query_file_as!(
        GrantRow,
        "src/oauth/queries/grant_insert.sql",
        grant.user_id.as_uuid(),
        grant.client_id,
        grant.client_name,
        grant.client_host,
        grant.provenance.as_str(),
        grant.scope.as_str(),
        grant.resource,
        grant.created_at,
    )
    .fetch_one(&mut *conn)
    .await?
    .try_into()
}
