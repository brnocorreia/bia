//! Extractors whose rejections are [`ApiError`]s, so a malformed request gets the uniform body.

use axum::extract::FromRequest;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::{ApiError, ErrorCode};

/// `axum::Json`, rejecting with `malformed_request` or `request_too_large`.
#[derive(Debug, Clone, Copy, Default, FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        let status = rejection.status();
        let code = if status == StatusCode::PAYLOAD_TOO_LARGE {
            ErrorCode::RequestTooLarge
        } else {
            ErrorCode::MalformedRequest
        };
        // axum's text names the problem and where, e.g. "missing field `id` at line 1". It can
        // quote a value from the body, so a route that takes Secret values must not use this.
        ApiError::new(status, code, rejection.body_text())
    }
}
