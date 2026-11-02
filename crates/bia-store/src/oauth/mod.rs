//! OAuth for MCP (ADR-0067): clients that registered themselves, authorization requests, Grants
//! and refresh tokens. Like Tokens, authorization codes and refresh tokens are stored as
//! SHA-256 hashes; minting and hashing them is the caller's job. A Grant's access tokens are
//! ordinary Token rows (`origin = oauth`) that point back at it.

mod model;
mod repository;
mod row;

pub use model::{
    Authorization, Client, CodeRedemption, Grant, GrantScope, NewApproval, NewAuthorization,
    NewClient, NewGrant, Provenance, RefreshRedemption, SecretHash,
};
pub use repository::{OAuthRepository, PgOAuthRepository};
