//! Errors in the shape OAuth clients parse (RFC 6749 section 5.2), for the endpoints they call
//! directly: the token, revocation and registration endpoints.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

use crate::ApiError;

/// An OAuth error response.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct OAuthErrorBody {
    /// An OAuth error code such as `invalid_grant`, `invalid_request`, `invalid_scope`,
    /// `invalid_target` or `unsupported_grant_type`.
    error: String,
    /// Human-readable explanation. Its wording may change; match on `error`.
    error_description: String,
}

#[derive(Debug)]
pub(crate) struct OAuthError {
    status: StatusCode,
    error: &'static str,
    description: String,
}

impl OAuthError {
    pub(crate) fn new(error: &'static str, description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error,
            description: description.into(),
        }
    }

    pub(crate) fn invalid_request(description: impl Into<String>) -> Self {
        Self::new("invalid_request", description)
    }

    pub(crate) fn invalid_grant(description: impl Into<String>) -> Self {
        Self::new("invalid_grant", description)
    }
}

impl From<ApiError> for OAuthError {
    fn from(error: ApiError) -> Self {
        Self {
            status: error.status(),
            error: if error.status().is_server_error() {
                "server_error"
            } else {
                "invalid_request"
            },
            description: error.body().message.clone(),
        }
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            axum::Json(OAuthErrorBody {
                error: self.error.to_owned(),
                error_description: self.description,
            }),
        )
            .into_response();
        no_store(&mut response);
        response
    }
}

/// Responses that carry credentials, or answer about them, must not be cached (RFC 6749 5.1).
pub(crate) fn no_store(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
}
