//! WebAuthn: the relying party derived from `public_url`, and the ceremonies' lifetime
//! (ADR-0065).

use axum::http::StatusCode;
use chrono::TimeDelta;
use webauthn_rs::{Webauthn, WebauthnBuilder};

use crate::{ApiConfig, ApiError, ErrorCode};

/// How long a browser has between a ceremony's `begin` and `finish`.
pub(crate) const CEREMONY_LIFETIME: TimeDelta = TimeDelta::minutes(5);

/// A Session must have signed in this recently to remove a passkey.
pub(crate) const REAUTHENTICATION_WINDOW: TimeDelta = TimeDelta::minutes(10);

const RP_NAME: &str = "bia";

/// The relying party: its ID is `public_url`'s host, and the origins it accepts are `public_url`'s
/// and `ui_url`'s, so a response made on any other origin, or for another RP ID, fails
/// verification. A browser also refuses an RP ID that is not the page's host or a parent of it,
/// so a UI on another host than `public_url`'s cannot use passkeys.
pub(crate) fn webauthn(config: &ApiConfig) -> Result<Webauthn, ApiError> {
    let unavailable = || {
        ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::PasskeysUnavailable,
            "passkeys need `public_url` to be set, with a domain name or `localhost` as its host",
        )
    };
    let url = config.public_url.as_ref().ok_or_else(unavailable)?;
    let host = url.host_str().ok_or_else(unavailable)?;
    let mut builder = WebauthnBuilder::new(host, url)
        .map_err(|_| unavailable())?
        .rp_name(RP_NAME);
    if let Some(ui_url) = &config.ui_url {
        builder = builder.append_allowed_origin(ui_url);
    }
    builder.build().map_err(|_| unavailable())
}

pub(crate) fn reauthentication_required() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        ErrorCode::ReauthenticationRequired,
        "this needs a Session that signed in within the last 10 minutes; sign in again",
    )
}
